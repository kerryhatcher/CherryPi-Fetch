/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::path::PathBuf;

use servo::resources::{Resource, ResourceReaderMethods};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) static ERROR_TEMPLATE_READ: AtomicBool = AtomicBool::new(false);

/// A default resource reader that provides baked in resources.
pub struct DefaultResourceReader;

impl ResourceReaderMethods for DefaultResourceReader {
    fn sandbox_access_files(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn sandbox_access_files_dirs(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn read(&self, file: Resource) -> Vec<u8> {
        if matches!(
            file,
            Resource::BadCertHTML | Resource::NetErrorHTML | Resource::CrashHTML
        ) {
            ERROR_TEMPLATE_READ.store(true, Ordering::SeqCst);
        }
        match file {
            Resource::BluetoothBlocklist => {
                &include_bytes!("servo_resources/gatt_blocklist.txt")[..]
            }
            Resource::DomainList => &include_bytes!("servo_resources/public_domains.txt")[..],
            Resource::HstsPreloadList => &include_bytes!("servo_resources/hsts_preload.fstmap")[..],
            Resource::BadCertHTML => &include_bytes!("servo_resources/badcert.html")[..],
            Resource::NetErrorHTML => &include_bytes!("servo_resources/neterror.html")[..],
            Resource::BrokenImageIcon => &include_bytes!("servo_resources/rippy.png")[..],
            Resource::CrashHTML => &include_bytes!("servo_resources/crash.html")[..],
            Resource::DirectoryListingHTML => {
                &include_bytes!("servo_resources/directory-listing.html")[..]
            }
            Resource::AboutMemoryHTML => &include_bytes!("servo_resources/about-memory.html")[..],
            Resource::DebuggerJS => &include_bytes!("servo_resources/debugger.js")[..],
            Resource::JsonViewerHTML => &include_bytes!("servo_resources/json-viewer.html")[..],
        }
        .to_owned()
    }
}

servo::submit_resource_reader!(&DefaultResourceReader);
