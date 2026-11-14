// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{
    Case,
    build::Build,
    vm::{self, Process},
};
use crate::Target;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

#[derive(Default)]
pub(super) struct Resources {
    pub arguments: Vec<String>,
    pub initrd: Option<PathBuf>,
    pub inputs: Vec<(Vec<u8>, Vec<u8>)>,
    pub shutdown: bool,
    peer: Option<Process>,
    monitor: Option<SocketPath>,
    filesystem: bool,
}

struct SocketPath(PathBuf);

impl Drop for SocketPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Resources {
    pub fn prepare(build: &mut Build, case: &Case, directory: &Path) -> Result<Self, String> {
        let mut resources = Self::default();
        let Some(suite) = case.name.strip_prefix("interrupt::") else {
            return Ok(resources);
        };
        resources.initrd = match suite {
            "controller" => None,
            "init" | "linux" | "net_service" => Some(build.archive(suite)?),
            "service" => Some(build.binary("devmgr")?),
            "dma" | "dma_drop" | "dma_revoke" => Some(build.program("runtime", "dma")?),
            "uart_driver" => Some(build.program("devmgr", "uart")?),
            "block" | "rng" | "rtc" | "power" | "net" => Some(build.program("devmgr", suite)?),
            "fs" => Some(build.program("fs", "ext4")?),
            "logger" => Some(build.program("logger", "service")?),
            "lwip" => Some(build.program("lwip", "loopback")?),
            "device" | "pci_firmware" | "pci_exhausted" | "pci_rollback" => {
                Some(build.program("runtime", "device")?)
            }
            _ => return Err(format!("no execution environment for {}", case.name)),
        };
        match suite {
            "dma" | "dma_drop" | "dma_revoke" => resources.dma(build.target),
            "device" | "pci_firmware" | "pci_exhausted" | "pci_rollback" => {
                resources.pci(build.target, suite)
            }
            "block" => resources.block(directory)?,
            "fs" => {
                build.fixture("prepare", directory)?;
                resources.filesystem = true;
                resources.ext4_drives(directory, false);
            }
            "linux" | "logger" => {
                build.fixture("volume", directory)?;
                resources.drive("disk", &directory.join("volume.ext4"), "");
                resources.device("virtio-rng-pci,disable-legacy=on");
                resources.inputs = if suite == "linux" {
                    linux_inputs()
                } else {
                    vec![(
                        b"Logger shared console RX\n".to_vec(),
                        b"shared-console-input".to_vec(),
                    )]
                };
            }
            "uart_driver" => {
                resources.inputs = [
                    ("UART RX 1\n", "first-uart"),
                    ("UART RX 2\n", "second-uart"),
                    ("UART service RX 1\n", "service-uart"),
                    ("UART service RX 2\n", "reconnected-uart"),
                ]
                .into_iter()
                .map(|(marker, input)| (marker.as_bytes().to_vec(), input.as_bytes().to_vec()))
                .collect()
            }
            "net" => resources.network(build, directory)?,
            "rng" => {
                resources.device("virtio-rng-pci,disable-legacy=on");
                if matches!(build.target, Target::Riscv64) {
                    resources.device("virtio-rng-device");
                }
            }
            "init" | "service" | "lwip" | "net_service" => {
                resources.device("virtio-rng-pci,disable-legacy=on")
            }
            "power" => resources.shutdown = true,
            _ => (),
        }
        Ok(resources)
    }

    pub fn finish(
        &mut self,
        build: &Build,
        kernel: &Path,
        directory: &Path,
        case: &Case,
        timeout: Duration,
    ) -> Result<(), String> {
        if self.filesystem {
            let reboot = directory.join("reboot");
            fs::create_dir(&reboot).map_err(|error| error.to_string())?;
            let mut resources = Self {
                initrd: self.initrd.clone(),
                ..Self::default()
            };
            resources.drive("fs", &directory.join("disk.raw"), "");
            vm::run(
                build.target,
                &build.root,
                kernel,
                &reboot,
                case,
                &mut resources,
                timeout,
            )?;
            build.fixture("recover", directory)?;
            let recovery = directory.join("recovery");
            fs::create_dir(&recovery).map_err(|error| error.to_string())?;
            resources.arguments.clear();
            resources.ext4_drives(directory, true);
            vm::run(
                build.target,
                &build.root,
                kernel,
                &recovery,
                case,
                &mut resources,
                timeout,
            )?;
            build.fixture("check", directory)?;
        }
        if let Some(mut peer) = self.peer.take() {
            let started = std::time::Instant::now();
            loop {
                if let Some(status) = peer.0.try_wait().map_err(|error| error.to_string())? {
                    if !status.success() {
                        return Err(format!("Ethernet peer failed: {status}"));
                    }
                    break;
                }
                if started.elapsed() >= Duration::from_secs(5) {
                    return Err("Ethernet peer did not stop".to_owned());
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(())
    }

    fn device(&mut self, value: &str) {
        self.arguments
            .extend(["-device".to_owned(), value.to_owned()]);
    }

    fn drive(&mut self, name: &str, path: &Path, options: &str) {
        self.arguments.extend([
            "-drive".to_owned(),
            format!(
                "if=none,id={name},format=raw,file={}{options}",
                path.display()
            ),
        ]);
        self.device(&format!("virtio-blk-pci,disable-legacy=on,drive={name}"));
    }

    fn faulty_drive(&mut self, name: &str, directory: &Path) {
        self.arguments.extend([
            "-drive".to_owned(),
            format!(
                "if=none,id={name},format=raw,file=blkdebug:{}:{}",
                directory.join(format!("{name}.conf")).display(),
                directory.join(format!("{name}.raw")).display()
            ),
        ]);
        self.device(&format!("virtio-blk-pci,disable-legacy=on,drive={name}"));
    }

    fn block(&mut self, directory: &Path) -> Result<(), String> {
        for (name, size, byte) in [("rw", 8, 0), ("ro", 4, 0x39), ("error", 2, 0)] {
            fs::write(
                directory.join(format!("{name}.raw")),
                vec![byte; size * 1024 * 1024],
            )
            .map_err(|error| error.to_string())?;
        }
        fs::write(directory.join("error.conf"), "[inject-error]\nevent = \"read_aio\"\nerrno = \"5\"\nimmediately = \"on\"\nonce = \"off\"\n").map_err(|error| error.to_string())?;
        self.drive("rw", &directory.join("rw.raw"), "");
        self.arguments.extend([
            "-drive".to_owned(),
            format!(
                "if=none,id=ro,format=raw,readonly=on,file={}",
                directory.join("ro.raw").display()
            ),
        ]);
        self.device("virtio-blk-pci,disable-legacy=on,drive=ro,logical_block_size=4096,physical_block_size=4096");
        self.faulty_drive("error", directory);
        Ok(())
    }

    fn ext4_drives(&mut self, directory: &Path, recovery: bool) {
        if recovery {
            for name in ["write", "flush", "prepare", "damaged"] {
                self.drive(name, &directory.join(format!("{name}.raw")), "");
            }
            self.arguments.extend([
                "-drive".to_owned(),
                format!(
                    "if=none,id=replay,format=raw,file=blkdebug:{}:{}",
                    directory.join("write.conf").display(),
                    directory.join("replay.raw").display()
                ),
            ]);
            self.device("virtio-blk-pci,disable-legacy=on,drive=replay");
        } else {
            self.drive("fs", &directory.join("disk.raw"), "");
            self.drive("ro", &directory.join("ro.raw"), ",readonly=on");
            for name in ["write", "flush", "prepare"] {
                self.faulty_drive(name, directory);
            }
        }
    }

    fn dma(&mut self, target: Target) {
        for device in [
            "virtio-rng-pci,disable-legacy=on,addr=1",
            "pci-bridge,id=dma_bridge,chassis_nr=1,addr=2",
            "virtio-rng-pci,disable-legacy=on,bus=dma_bridge,addr=1",
            "virtio-rng-pci,disable-legacy=on,bus=dma_bridge,addr=2",
        ] {
            self.device(device);
        }
        if matches!(target, Target::Riscv64) {
            self.device("virtio-rng-device");
        }
    }

    fn pci(&mut self, target: Target, suite: &str) {
        self.device(if matches!(target, Target::Riscv64) {
            "pci-testdev,membar=1M,addr=1"
        } else {
            "virtio-serial-pci,disable-legacy=on,addr=1"
        });
        for device in [
            "pci-bridge,id=bridge1,chassis_nr=1,addr=2",
            "e1000,bus=bridge1,addr=1",
            "virtio-serial-pci,disable-legacy=on,bus=bridge1,addr=2",
            "e1000,addr=7",
        ] {
            self.device(device);
        }
        if suite == "pci_exhausted" {
            self.device("virtio-gpu-pci,hostmem=1G,addr=8");
        }
    }

    fn network(&mut self, build: &Build, directory: &Path) -> Result<(), String> {
        let path = std::env::temp_dir().join(format!(
            "arona-test-{}-{}.qmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        self.arguments.extend([
            "-qmp".to_owned(),
            format!("unix:{},server=on,wait=off", path.display()),
        ]);
        let log = File::create(directory.join("peer.log")).map_err(|error| error.to_string())?;
        let mut child = Process(
            Command::new("python3")
                .arg(build.root.join("components/devmgr/tests/net/peer.py"))
                .args(["--port", "0", "--qmp"])
                .arg(&path)
                .stdout(Stdio::piped())
                .stderr(log.try_clone().map_err(|error| error.to_string())?)
                .spawn()
                .map_err(|error| error.to_string())?,
        );
        let output = child.0.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut output = BufReader::new(output);
            let mut line = String::new();
            let result = output
                .read_line(&mut line)
                .map(|_| line.clone())
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            let mut log = log;
            let _ = log.write_all(line.as_bytes());
            let _ = std::io::copy(&mut output, &mut log);
        });
        let line = receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "Ethernet peer did not become ready")??;
        let port = line
            .trim()
            .strip_prefix("Ethernet peer ready ")
            .ok_or("invalid Ethernet peer startup")?
            .parse::<u16>()
            .map_err(|_| "invalid Ethernet peer port")?;
        self.arguments.extend([
            "-netdev".to_owned(),
            format!("socket,id=net0,connect=127.0.0.1:{port}"),
        ]);
        self.device("virtio-net-pci,id=nic,disable-legacy=on,netdev=net0,mac=52:54:00:12:34:56");
        self.peer = Some(child);
        self.monitor = Some(SocketPath(path));
        Ok(())
    }
}

fn linux_inputs() -> Vec<(Vec<u8>, Vec<u8>)> {
    let inputs: &[(&[u8], &[u8])] = &[
        (b"Linux terminal utf8", b"A\xe2\x82\xac\x7fB\n"),
        (b"Linux terminal bytes", b"A\xe2\x82\xac\x7fB\n"),
        (b"Linux terminal invalid", b"A\xe2\x82\x7fB\n"),
        (b"Linux terminal reprint", b"ab\x12\n"),
        (b"Linux terminal canonical", b"abX\x7fc\n"),
        (b"Linux terminal raw", b"raw-data"),
        (b"Linux terminal minimum", b"1234"),
        (b"Linux terminal interbyte", b"xy"),
        (b"Linux terminal injected Z", b"Y"),
        (b"Linux terminal signal", b"\x03\x1c\x1adone\n"),
        (b"Linux terminal suspend", b"\x1a"),
        (b"Linux terminal resume", b"ok\n"),
    ];
    inputs
        .iter()
        .chain(inputs)
        .map(|(marker, input)| (marker.to_vec(), input.to_vec()))
        .collect()
}
