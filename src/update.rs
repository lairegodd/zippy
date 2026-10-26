use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::thread::sleep;
use std::time::Duration;

use windows::Data::Json::JsonObject;
use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts,
};
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::core::{Error, HSTRING, PCWSTR, h, w};

const REPO: &str = "lairegodd/zippy";
const ASSET: &str = "zippy.exe";
const MAX_DOWNLOAD: usize = 32 << 20;

#[derive(Debug)]
pub struct Release {
    pub tag: String,
    url: String,
    sha256: String,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.').map(|p| p.parse().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// Returns the latest GitHub release when it is newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    let body = http_get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?;
    let text = String::from_utf8(body).map_err(|_| "GitHub sent a reply that is not text".to_owned())?;
    newer_release(&text, current_version())
}

fn newer_release(json: &str, current: &str) -> Result<Option<Release>, String> {
    let bad = |e: Error| format!("unexpected reply from GitHub: {e}");
    let release = JsonObject::Parse(&HSTRING::from(json)).map_err(bad)?;
    let tag = release.GetNamedString(h!("tag_name")).map_err(bad)?.to_string();
    let latest = parse_version(&tag).ok_or_else(|| format!("the latest release has an unknown tag `{tag}`"))?;
    let current = parse_version(current).ok_or_else(|| format!("this build has an unknown version `{current}`"))?;
    if latest <= current {
        return Ok(None);
    }
    let assets = release.GetNamedArray(h!("assets")).map_err(bad)?;
    for i in 0..assets.Size().map_err(bad)? {
        let asset = assets.GetObjectAt(i).map_err(bad)?;
        if asset.GetNamedString(h!("name")).map_err(bad)? != ASSET {
            continue;
        }
        let url = asset.GetNamedString(h!("browser_download_url")).map_err(bad)?.to_string();
        let digest = asset.GetNamedString(h!("digest")).map(|d| d.to_string()).unwrap_or_default();
        let sha256 = digest
            .strip_prefix("sha256:")
            .filter(|d| d.len() == 64)
            .ok_or_else(|| format!("{tag} has no SHA-256 checksum for {ASSET}; download it from GitHub instead"))?;
        return Ok(Some(Release { tag, url, sha256: sha256.to_ascii_lowercase() }));
    }
    Err(format!("{tag} has no {ASSET} to download"))
}

fn old_exe(exe: &Path) -> PathBuf {
    exe.with_extension("old.exe")
}

/// Windows can rename a running exe but not delete it, so the old one is swapped aside and removed on the next start.
pub fn install(release: &Release) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate zippy.exe: {e}"))?;
    let bytes = http_get(&release.url)?;
    let hash = sha256_hex(&bytes)?;
    if hash != release.sha256 {
        return Err(format!("the download does not match its checksum ({hash}); nothing was changed"));
    }
    let new = exe.with_extension("new.exe");
    let old = old_exe(&exe);
    fs::write(&new, &bytes).map_err(|e| format!("cannot write {}: {e}", new.display()))?;
    let _ = fs::remove_file(&old);
    if let Err(e) = fs::rename(&exe, &old) {
        let _ = fs::remove_file(&new);
        return Err(format!("cannot replace {}: {e}", exe.display()));
    }
    if let Err(e) = fs::rename(&new, &exe) {
        let _ = fs::rename(&old, &exe);
        let _ = fs::remove_file(&new);
        return Err(format!("cannot replace {}: {e}", exe.display()));
    }
    Ok(exe)
}

/// Right after an update the previous exe may still be exiting, so retry briefly.
pub fn remove_previous() {
    let Ok(exe) = std::env::current_exe() else { return };
    let old = old_exe(&exe);
    for _ in 0..20 {
        if !old.exists() || fs::remove_file(&old).is_ok() {
            return;
        }
        sleep(Duration::from_millis(100));
    }
}

fn sha256_hex(data: &[u8]) -> Result<String, String> {
    let mut digest = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut digest) }
        .ok()
        .map_err(|e| format!("cannot hash the download: {e}"))?;
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn split_url(url: &str) -> Option<(&str, String)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    (!host.is_empty()).then(|| (host, format!("/{path}")))
}

struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void) -> Result<Self, Error> {
        if raw.is_null() { Err(Error::from_win32()) } else { Ok(Self(raw)) }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// HTTPS GET through WinHTTP, which follows GitHub's redirect to its download host.
fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let (host, path) = split_url(url).ok_or_else(|| format!("unsupported URL {url}"))?;
    let net = |e: Error| format!("cannot reach {host}: {e}");
    unsafe {
        let agent = HSTRING::from(format!("zippy/{}", current_version()));
        let session = Handle::new(WinHttpOpen(
            &agent,
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))
        .map_err(net)?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 30_000).map_err(net)?;
        let connect =
            Handle::new(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0)).map_err(net)?;
        let request = Handle::new(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))
        .map_err(net)?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(net)?;
        WinHttpReceiveResponse(request.0, ptr::null_mut()).map_err(net)?;

        let mut status = 0u32;
        let mut size = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            ptr::null_mut(),
        )
        .map_err(net)?;
        if status != 200 {
            return Err(format!("{host} answered HTTP {status}"));
        }

        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available).map_err(net)?;
            if available == 0 {
                return Ok(body);
            }
            let start = body.len();
            if start + available as usize > MAX_DOWNLOAD {
                return Err(format!("the reply from {host} is larger than {} MB", MAX_DOWNLOAD >> 20));
            }
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(request.0, body[start..].as_mut_ptr().cast(), available, &mut read).map_err(net)?;
            body.truncate(start + read as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "78463996730826ea04790c0d19285f06b09119095c723a6a65c6f57f72128924";

    fn release_json(tag: &str, asset: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","assets":[{{"name":"notes.txt","browser_download_url":"https://x/n","digest":null}},{asset}]}}"#
        )
    }

    fn exe_asset(digest: &str) -> String {
        format!(r#"{{"name":"zippy.exe","browser_download_url":"https://github.com/{REPO}/releases/download/v1.2.0/zippy.exe","digest":{digest}}}"#)
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v1.10.0"), Some((1, 10, 0)));
        assert!(parse_version("v1.10.0") > parse_version("1.9.9"));
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn only_a_newer_release_with_a_checksummed_exe_is_offered() {
        let digest = format!(r#""sha256:{DIGEST}""#);
        let release = newer_release(&release_json("v1.2.0", &exe_asset(&digest)), "1.1.0").unwrap().unwrap();
        assert_eq!(release.tag, "v1.2.0");
        assert!(release.url.ends_with("/v1.2.0/zippy.exe"));
        assert_eq!(release.sha256, DIGEST);

        assert!(newer_release(&release_json("v1.1.0", &exe_asset(&digest)), "1.1.0").unwrap().is_none());
        assert!(newer_release(&release_json("v1.0.0", &exe_asset(&digest)), "1.1.0").unwrap().is_none());
        assert!(newer_release(&release_json("v1.2.0", &exe_asset("null")), "1.1.0").unwrap_err().contains("checksum"));
        assert!(newer_release(&release_json("v1.2.0", r#"{"name":"other.zip"}"#), "1.1.0").is_err());
    }

    #[test]
    fn url_splits_into_host_and_path() {
        assert_eq!(split_url("https://api.github.com/repos/a/b"), Some(("api.github.com", "/repos/a/b".to_owned())));
        assert_eq!(split_url("https://example.com"), Some(("example.com", "/".to_owned())));
        assert_eq!(split_url("http://example.com/x"), None);
    }
}
