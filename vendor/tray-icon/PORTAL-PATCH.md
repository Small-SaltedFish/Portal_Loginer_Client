Based on tray-icon 0.19.2 from crates.io, archive SHA-256 d48a05076dd272615d03033bf04f480199f7d1b66a8ac64d75c625fc4a70c06b.
Windows NOTIFYICONDATAW calls set cbSize explicitly. Failed icon registration destroys its temporary window before returning the original OS error, so startup retries do not leak windows/user data. No authentication behavior changes.
Licenses: Apache-2.0 OR MIT; see LICENSE-APACHE and LICENSE-MIT.
