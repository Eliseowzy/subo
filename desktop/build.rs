fn main() {
    // Windows 清单在默认的基础上开长路径支持，见 windows/app.manifest
    let windows = tauri_build::WindowsAttributes::new().app_manifest(include_str!("windows/app.manifest"));
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows)).expect("tauri-build 失败");
}
