// SPDX-License-Identifier: MPL-2.0

//! 查询设备身份与带来源命名空间的属性。

use runtime::{
    DeviceSet, Handle,
    device::{DeviceField, DeviceInfo},
};

use crate::Error;

pub struct DeviceDescription<'a> {
    catalog: &'a Handle<DeviceSet>,
    info: DeviceInfo,
}

impl<'a> DeviceDescription<'a> {
    pub fn new(catalog: &'a Handle<DeviceSet>, id: u32) -> Result<Self, Error> {
        Ok(Self {
            catalog,
            info: catalog.describe(id)?,
        })
    }

    pub fn info(&self) -> DeviceInfo {
        self.info
    }

    pub fn matches(&self, namespace: &str, identity: &str) -> Result<bool, Error> {
        let mut buffer = [0; 128];
        for index in 0..self.info.identities {
            if self.catalog.read(
                self.info.id,
                DeviceField::IdentityNamespace,
                index,
                &mut buffer,
            )? != namespace.as_bytes()
            {
                continue;
            }
            if self
                .catalog
                .read(self.info.id, DeviceField::Identity, index, &mut buffer)?
                == identity.as_bytes()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn property<'b>(
        &self,
        namespace: &str,
        name: &str,
        output: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Error> {
        let mut buffer = [0; 128];
        for index in 0..self.info.properties {
            if self.catalog.read(
                self.info.id,
                DeviceField::PropertyNamespace,
                index,
                &mut buffer,
            )? != namespace.as_bytes()
                || self
                    .catalog
                    .read(self.info.id, DeviceField::PropertyName, index, &mut buffer)?
                    != name.as_bytes()
            {
                continue;
            }
            return Ok(Some(self.catalog.read(
                self.info.id,
                DeviceField::PropertyValue,
                index,
                output,
            )?));
        }
        Ok(None)
    }

    pub(crate) fn resolve(&self, namespace: &str, name: &str, value: &[u8]) -> Result<Self, Error> {
        let mut cursor = 0;
        let mut found = None;
        let mut buffer = [0; 128];
        while let Some(id) = self.catalog.next(cursor)? {
            let device = Self::new(self.catalog, id)?;
            if device.property(namespace, name, &mut buffer)? == Some(value) {
                if found.is_some() {
                    return Err(Error::InvalidConfiguration);
                }
                found = Some(device);
            }
            cursor = id;
        }
        found.ok_or(Error::InvalidConfiguration)
    }
}
