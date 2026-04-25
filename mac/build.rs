// Windows-only build step: embed an application manifest that asks
// for admin elevation up front. Without it, double-clicking
// vm_ctl_app.exe (or vm_ctl.exe) from Explorer runs as the current
// non-admin user and the QEMU/sudo/service code paths fail.
// "Run as administrator" via right-click also isn't available on
// some surfaces (Parallels psf shares, mapped drives), so embedding
// the manifest is the only reliable way.
fn main() {
    #[cfg(windows)]
    {
        use embed_manifest::manifest::ExecutionLevel;
        use embed_manifest::{embed_manifest, new_manifest};
        if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
            embed_manifest(
                new_manifest("vmcontrol")
                    .requested_execution_level(ExecutionLevel::RequireAdministrator),
            )
            .expect("unable to embed Windows manifest");
        }
    }
}
