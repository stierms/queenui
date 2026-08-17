use rcgen::{generate_simple_self_signed, CertifiedKey};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path};

pub const CERTIFICATE_FILE: &str = "runner-cert.der";
pub const PRIVATE_KEY_FILE: &str = "runner-key.der";

#[derive(Clone, Debug)]
pub struct TlsIdentity {
    pub certificate_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
    pub fingerprint: [u8; 32],
}

pub fn ensure(data_dir: &Path) -> Result<TlsIdentity, String> {
    fs::create_dir_all(data_dir)
        .map_err(|error| format!("Could not create the runner data directory: {error}"))?;
    protect_directory(data_dir)?;
    let certificate_path = data_dir.join(CERTIFICATE_FILE);
    let private_key_path = data_dir.join(PRIVATE_KEY_FILE);
    match (certificate_path.exists(), private_key_path.exists()) {
        (true, true) => load(&certificate_path, &private_key_path),
        (false, false) => create(data_dir, &certificate_path, &private_key_path),
        _ => Err(
            "Runner TLS identity is incomplete; restore both the certificate and private key or initialize a fresh data directory"
                .into(),
        ),
    }
}

fn create(
    data_dir: &Path,
    certificate_path: &Path,
    private_key_path: &Path,
) -> Result<TlsIdentity, String> {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into(), "::1".into()])
            .map_err(|error| format!("Could not generate the runner TLS identity: {error}"))?;
    let identity = TlsIdentity {
        certificate_der: cert.der().as_ref().to_vec(),
        private_key_der: signing_key.serialize_der(),
        fingerprint: Sha256::digest(cert.der().as_ref()).into(),
    };
    let stored_private_key = protect_private_key(&identity.private_key_der)?;

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let temporary_key = data_dir.join(format!(".{PRIVATE_KEY_FILE}.{suffix}.tmp"));
    let temporary_cert = data_dir.join(format!(".{CERTIFICATE_FILE}.{suffix}.tmp"));
    write_new(&temporary_key, &stored_private_key, 0o600)?;
    if let Err(error) = write_new(&temporary_cert, &identity.certificate_der, 0o644) {
        let _ = fs::remove_file(&temporary_key);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary_key, private_key_path) {
        let _ = fs::remove_file(&temporary_key);
        let _ = fs::remove_file(&temporary_cert);
        return Err(format!("Could not install the runner private key: {error}"));
    }
    if let Err(error) = fs::rename(&temporary_cert, certificate_path) {
        let _ = fs::remove_file(private_key_path);
        let _ = fs::remove_file(&temporary_cert);
        return Err(format!("Could not install the runner certificate: {error}"));
    }
    sync_directory(data_dir)?;
    load(certificate_path, private_key_path)
}

fn load(certificate_path: &Path, private_key_path: &Path) -> Result<TlsIdentity, String> {
    enforce_private_key_permissions(private_key_path)?;
    let certificate_der = fs::read(certificate_path)
        .map_err(|error| format!("Could not read the runner certificate: {error}"))?;
    let stored_private_key = fs::read(private_key_path)
        .map_err(|error| format!("Could not read the runner private key: {error}"))?;
    let private_key_der = unprotect_private_key(&stored_private_key)?;
    if certificate_der.is_empty() || private_key_der.is_empty() {
        return Err("Runner TLS identity files must not be empty".into());
    }
    Ok(TlsIdentity {
        fingerprint: Sha256::digest(&certificate_der).into(),
        certificate_der,
        private_key_der,
    })
}

fn write_new(path: &Path, bytes: &[u8], unix_mode: u32) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(unix_mode);
    }
    #[cfg(not(unix))]
    let _ = unix_mode;
    let mut file = options
        .open(path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not persist {}: {error}", path.display()))
}

#[cfg(unix)]
fn protect_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Could not protect the runner data directory: {error}"))
}

#[cfg(not(unix))]
fn protect_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn enforce_private_key_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|error| format!("Could not inspect the runner private key: {error}"))?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o600 {
        return Err(format!(
            "Runner private key {} must have mode 0600 (found {mode:04o})",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn enforce_private_key_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
fn protect_private_key(bytes: &[u8]) -> Result<Vec<u8>, String> {
    Ok(bytes.to_vec())
}

#[cfg(not(windows))]
fn unprotect_private_key(bytes: &[u8]) -> Result<Vec<u8>, String> {
    Ok(bytes.to_vec())
}

#[cfg(windows)]
fn protect_private_key(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use std::{ptr, slice};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB},
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len())
            .map_err(|_| "The runner private key is too large for DPAPI".to_string())?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let success = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if success == 0 {
        return Err(format!(
            "Could not protect the runner private key with Windows DPAPI: {}",
            std::io::Error::last_os_error()
        ));
    }
    if output.pbData.is_null() || output.cbData == 0 {
        unsafe {
            LocalFree(output.pbData.cast());
        }
        return Err("Windows DPAPI returned an empty protected private key".into());
    }
    let protected =
        unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(protected)
}

#[cfg(windows)]
fn unprotect_private_key(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use std::{ptr, slice};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len())
            .map_err(|_| "The protected runner private key is too large".to_string())?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let success = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if success == 0 {
        return Err(format!(
            "Could not unlock the runner private key with Windows DPAPI: {}",
            std::io::Error::last_os_error()
        ));
    }
    if output.pbData.is_null() || output.cbData == 0 {
        unsafe {
            LocalFree(output.pbData.cast());
        }
        return Err("Windows DPAPI returned an empty runner private key".into());
    }
    let plaintext =
        unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(plaintext)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Could not flush the runner data directory: {error}"))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

pub fn fingerprint_hex(fingerprint: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(64);
    for byte in fingerprint {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{ensure, CERTIFICATE_FILE, PRIVATE_KEY_FILE};
    use std::{fs, path::PathBuf};

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!("queen-runner-identity-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn private_key_is_separate_and_owner_only() {
        let directory = temporary_directory();
        let identity = ensure(&directory).unwrap();
        assert!(!identity.certificate_der.is_empty());
        assert!(!identity.private_key_der.is_empty());
        assert_ne!(
            directory.join(CERTIFICATE_FILE),
            directory.join(PRIVATE_KEY_FILE)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory.join(PRIVATE_KEY_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(directory.join(CERTIFICATE_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o644
            );
        }
        #[cfg(windows)]
        assert_ne!(
            fs::read(directory.join(PRIVATE_KEY_FILE)).unwrap(),
            identity.private_key_der
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
