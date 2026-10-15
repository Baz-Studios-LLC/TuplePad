//! Signs TuplePad's release downloads, so the app's updater can tell they're genuine.
//!
//!     cargo run --example update_sign -- keygen <secret key file>
//!     cargo run --example update_sign -- sign <secret key file> <download> "<trusted comment>"
//!
//! `keygen` writes a new secret key (keep it out of the repository) and prints the public key,
//! which goes in `src/update.rs`. `sign` prints a minisign signature; the trusted comment names
//! the version and system ("TuplePad v002 mac"), and the updater checks it.

use std::fs::File;
use std::io::{BufReader, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen", path] => {
            if std::path::Path::new(path).exists() {
                return Err(format!("{path} already exists").into());
            }
            let pair = minisign::KeyPair::generate_unencrypted_keypair()?;
            let secret = pair.sk.to_box(Some("TuplePad update signing key"))?.into_string();
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(path)?.write_all(secret.as_bytes())?;
            println!("{}", pair.pk.to_base64());
        }
        ["sign", key, file, comment] => {
            let secret = minisign::SecretKeyBox::from_string(&std::fs::read_to_string(key)?)?.into_unencrypted_secret_key()?;
            let public = minisign::PublicKey::from_secret_key(&secret)?;
            let data = BufReader::new(File::open(file)?);
            let signature = minisign::sign(Some(&public), &secret, data, Some(comment), Some("TuplePad release signature"))?;
            print!("{}", signature.into_string());
        }
        ["public", key] => {
            let secret = minisign::SecretKeyBox::from_string(&std::fs::read_to_string(key)?)?.into_unencrypted_secret_key()?;
            println!("{}", minisign::PublicKey::from_secret_key(&secret)?.to_base64());
        }
        _ => return Err("usage: update_sign keygen <key> | sign <key> <file> <trusted comment> | public <key>".into()),
    }
    Ok(())
}
