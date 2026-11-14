#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

"""Ethernet echo peer for the real VirtIO net component scenario.

Connect QEMU's socket netdev to --port and expose its QMP Unix socket at --qmp.
Use device id=nic. No IP stack or privileged host interface is required.
"""

import argparse
import json
import socket
import struct
import time


class Peer:
    def __init__(self, network, monitor):
        self.network = network
        self.monitor = monitor.makefile('rwb', buffering=0)
        assert 'QMP' in json.loads(self.monitor.readline())
        self.command('qmp_capabilities')
        self.packets = 0
        self.injected = False
        self.link_deadline = None

    def command(self, command, arguments=None):
        self.monitor.write(json.dumps({'execute': command, 'arguments': arguments or {}}).encode() + b'\n')
        while True:
            response = json.loads(self.monitor.readline())
            if 'return' in response:
                return response['return']
            if 'error' in response:
                raise RuntimeError(response['error'])

    def send(self, frame):
        self.network.sendall(struct.pack('!I', len(frame)) + frame)

    def run(self):
        pending = bytearray()
        self.network.settimeout(0.1)
        while True:
            if self.link_deadline is not None and time.monotonic() >= self.link_deadline:
                self.command('set_link', {'name': 'nic', 'up': True})
                self.link_deadline = None
            try:
                data = self.network.recv(65536)
            except socket.timeout:
                continue
            if not data:
                return
            pending.extend(data)
            while len(pending) >= 4:
                length = struct.unpack('!I', pending[:4])[0]
                if length > 65536:
                    raise ValueError('invalid QEMU frame length')
                if len(pending) < length + 4:
                    break
                frame = bytes(pending[4:length + 4])
                del pending[:length + 4]
                self.frame(frame)

    def frame(self, frame):
        assert len(frame) >= 60, 'guest must pad short Ethernet frames'
        assert frame[:6] == bytes.fromhex('525400654321')
        assert frame[12:14] == b'\x88\xb5'
        self.packets += 1
        sequence = int.from_bytes(frame[14:18], 'little')
        if sequence == 0x40000:
            self.command('set_link', {'name': 'nic', 'up': False})
            self.link_deadline = time.monotonic() + 3
            return
        # Avoid delayed echoes crossing the explicit reset/session closure checkpoints.
        if sequence in (0x10000, 0x50000):
            return
        echo = frame[6:12] + frame[:6] + frame[12:]
        if not self.injected:
            self.send(echo[:8])
            self.send(echo + b'\x00')
            self.injected = True
        self.send(echo)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=12345)
    parser.add_argument('--qmp', required=True)
    arguments = parser.parse_args()
    with socket.socket() as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(('127.0.0.1', arguments.port))
        listener.listen(1)
        print(f'Ethernet peer ready {listener.getsockname()[1]}', flush=True)
        network, _ = listener.accept()
        with network, socket.socket(socket.AF_UNIX) as monitor:
            monitor.connect(arguments.qmp)
            peer = Peer(network, monitor)
            try:
                peer.run()
            except (ConnectionResetError, BrokenPipeError):
                pass
            print(f'Ethernet peer handled {peer.packets} frames', flush=True)


if __name__ == '__main__':
    main()
