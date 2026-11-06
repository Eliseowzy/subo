//! API Key 之类的机密：存系统钥匙串，不写进任何 JSON 配置。
//!
//! - macOS：钥匙串，经 `security` 命令。写入走 `security -i` 从标准输入读命令，机密不出现在进程参数里
//! - Linux：Secret Service，经 `secret-tool`（libsecret），机密同样从标准输入给
//! - Windows：凭据管理器（普通凭据，目标名 `<服务名>/<账号>`），直接调系统接口
//! - 都不可用（没装 secret-tool、没有桌面会话）时退回 `<subo 目录>/secrets.json`，权限 0600
//!
//! 都是阻塞调用（钥匙串可能弹授权框），在 `spawn_blocking` 里用。

use crate::{engine, library};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Stdio;

/// 机密存在哪；记在引用它的配置里，读的时候直接去那里找
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Store {
    /// 没存
    #[default]
    None,
    /// 系统钥匙串（macOS 钥匙串、Linux Secret Service 或 Windows 凭据管理器）
    Keychain,
    /// 退路：`<subo 目录>/secrets.json`
    File,
}

/// 钥匙串里的服务名。设了 `SUBO_HOME`（测试、多实例）时带上目录，各实例互不串
fn service() -> String {
    match std::env::var_os("SUBO_HOME") {
        Some(_) => format!("Subo ({})", library::subo_dir().display()),
        None => "Subo".into(),
    }
}

fn file() -> PathBuf {
    library::subo_dir().join("secrets.json")
}

/// 这台机器上系统钥匙串能不能用（只看命令在不在，真正存取时仍可能失败）
pub fn keychain_available() -> bool {
    if cfg!(target_os = "macos") {
        engine::which("security").is_some() || PathBuf::from("/usr/bin/security").is_file()
    } else if cfg!(windows) {
        true
    } else {
        engine::which("secret-tool").is_some()
    }
}

/// 存一条机密，返回实际存在哪。钥匙串失败就退回文件
pub fn set(account: &str, secret: &str) -> Result<Store, String> {
    if keychain_available() {
        match keychain_set(account, secret) {
            Ok(()) => {
                // 以前退回过文件的，换进钥匙串后把文件里那份删掉
                let _ = file_remove(account);
                return Ok(Store::Keychain);
            }
            Err(e) => eprintln!("[secret] 钥匙串写入失败，改存文件：{e}"),
        }
    }
    file_set(account, secret)?;
    Ok(Store::File)
}

pub fn get(account: &str, store: Store) -> Result<Option<String>, String> {
    match store {
        Store::None => Ok(None),
        Store::Keychain => keychain_get(account),
        Store::File => Ok(file_load().remove(account)),
    }
}

pub fn remove(account: &str, store: Store) {
    let r = match store {
        Store::None => Ok(()),
        Store::Keychain => keychain_remove(account),
        Store::File => file_remove(account),
    };
    if let Err(e) = r {
        eprintln!("[secret] 删除 {account} 失败：{e}");
    }
}

/* ---------- macOS 钥匙串 ---------- */

#[cfg(target_os = "macos")]
fn security() -> PathBuf {
    engine::which("security").unwrap_or_else(|| PathBuf::from("/usr/bin/security"))
}

/// `security -i` 的命令行：双引号里 `\` 和 `"` 要转义
#[cfg(target_os = "macos")]
fn sec_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
fn keychain_set(account: &str, secret: &str) -> Result<(), String> {
    if secret.contains(['\n', '\r']) || account.contains(['\n', '\r']) {
        return Err("含换行".into());
    }
    let line = format!(
        "add-generic-password -U -s {} -a {} -l {} -w {}\n",
        sec_quote(&service()),
        sec_quote(account),
        sec_quote(&service()),
        sec_quote(secret)
    );
    let mut child = crate::engine::command(security())
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child.stdin.take().unwrap().write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    // 交互模式下单条命令失败，退出码仍是 0，只能看错误输出
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() || !err.is_empty() {
        return Err(if err.is_empty() { format!("security 退出码 {:?}", out.status.code()) } else { err });
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn keychain_get(account: &str) -> Result<Option<String>, String> {
    let out = crate::engine::command(security())
        .args(["find-generic-password", "-s", &service(), "-a", account, "-w"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    match out.status.code() {
        Some(0) => Ok(Some(String::from_utf8_lossy(&out.stdout).trim_end_matches(['\n', '\r']).to_string())),
        // errSecItemNotFound
        Some(44) => Ok(None),
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
}

#[cfg(target_os = "macos")]
fn keychain_remove(account: &str) -> Result<(), String> {
    let out = crate::engine::command(security())
        .args(["delete-generic-password", "-s", &service(), "-a", account])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    match out.status.code() {
        Some(0) | Some(44) => Ok(()),
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
}

/* ---------- Linux Secret Service ---------- */

#[cfg(all(unix, not(target_os = "macos")))]
fn secret_tool() -> Result<PathBuf, String> {
    engine::which("secret-tool").ok_or_else(|| "没有 secret-tool".to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn keychain_set(account: &str, secret: &str) -> Result<(), String> {
    let mut child = crate::engine::command(secret_tool()?)
        .args(["store", "--label", &service(), "service", &service(), "account", account])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child.stdin.take().unwrap().write_all(secret.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn keychain_get(account: &str) -> Result<Option<String>, String> {
    let out = crate::engine::command(secret_tool()?)
        .args(["lookup", "service", &service(), "account", account])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    // 找不到时退出码 1、输出为空
    if out.status.success() {
        Ok(Some(String::from_utf8_lossy(&out.stdout).to_string()))
    } else if out.stderr.is_empty() {
        Ok(None)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn keychain_remove(account: &str) -> Result<(), String> {
    let out = crate::engine::command(secret_tool()?)
        .args(["clear", "service", &service(), "account", account])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() || out.stderr.is_empty() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/* ---------- Windows 凭据管理器 ---------- */

#[cfg(windows)]
fn cred_target(account: &str) -> Vec<u16> {
    format!("{}/{account}", service()).encode_utf16().chain([0]).collect()
}

#[cfg(windows)]
fn keychain_set(account: &str, secret: &str) -> Result<(), String> {
    use windows_sys::Win32::Security::Credentials::{CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC};
    let mut target = cred_target(account);
    let mut user: Vec<u16> = account.encode_utf16().chain([0]).collect();
    let mut blob = secret.as_bytes().to_vec();
    // SAFETY: 结构里的指针都指向本函数里活着的缓冲区，CredWriteW 只读不留
    let ok = unsafe {
        let mut c: CREDENTIALW = std::mem::zeroed();
        c.Type = CRED_TYPE_GENERIC;
        c.TargetName = target.as_mut_ptr();
        c.UserName = user.as_mut_ptr();
        c.CredentialBlobSize = blob.len() as u32;
        c.CredentialBlob = blob.as_mut_ptr();
        c.Persist = CRED_PERSIST_LOCAL_MACHINE;
        CredWriteW(&c, 0)
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[cfg(windows)]
fn keychain_get(account: &str) -> Result<Option<String>, String> {
    use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
    use windows_sys::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let target = cred_target(account);
    let mut p: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: 成功时系统分配 CREDENTIALW，读完用 CredFree 释放
    unsafe {
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == 0 {
            let e = std::io::Error::last_os_error();
            return if e.raw_os_error() == Some(ERROR_NOT_FOUND as i32) { Ok(None) } else { Err(e.to_string()) };
        }
        let c = &*p;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let out = String::from_utf8_lossy(bytes).into_owned();
        CredFree(p as *const _);
        Ok(Some(out))
    }
}

#[cfg(windows)]
fn keychain_remove(account: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
    use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
    let target = cred_target(account);
    // SAFETY: target 是以 0 结尾的宽字符串
    if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
            return Err(e.to_string());
        }
    }
    Ok(())
}

/* ---------- 退路：0600 文件 ---------- */

fn file_load() -> BTreeMap<String, String> {
    fs::read(file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn file_save(map: &BTreeMap<String, String>) -> Result<(), String> {
    let path = file();
    if map.is_empty() {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp).map_err(|e| e.to_string())?;
    f.write_all(&serde_json::to_vec_pretty(map).unwrap_or_default()).map_err(|e| e.to_string())?;
    drop(f);
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn file_set(account: &str, secret: &str) -> Result<(), String> {
    let mut map = file_load();
    map.insert(account.to_string(), secret.to_string());
    file_save(&map)
}

fn file_remove(account: &str) -> Result<(), String> {
    let mut map = file_load();
    if map.remove(account).is_some() {
        file_save(&map)?;
    }
    Ok(())
}
