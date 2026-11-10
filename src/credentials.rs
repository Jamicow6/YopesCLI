use std::io::{self, Write};

use anyhow::{Context, Result, anyhow, bail};
use argon2::Argon2;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, rand_core::RngCore},
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;

pub struct SealedSecret {
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

pub fn seal(password: &str, secret: &str) -> Result<SealedSecret> {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let key = derive_key(password, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| anyhow!("could not initialize the encryption cipher"))?;
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, secret.as_bytes())
        .map_err(|_| anyhow!("could not encrypt the API key"))?;
    Ok(SealedSecret {
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
}

pub fn open(password: &str, salt: &str, nonce: &str, ciphertext: &str) -> Result<String> {
    let salt = STANDARD
        .decode(salt)
        .context("invalid encrypted key salt")?;
    let nonce = STANDARD
        .decode(nonce)
        .context("invalid encrypted key nonce")?;
    let ciphertext = STANDARD
        .decode(ciphertext)
        .context("invalid encrypted key data")?;
    if salt.len() < 8 {
        bail!("encrypted key salt is too short");
    }
    if nonce.len() != NONCE_LEN {
        bail!("encrypted key nonce has an invalid length");
    }
    let key = derive_key(password, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| anyhow!("could not initialize the decryption cipher"))?;
    let plaintext = cipher
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_slice())
        .map_err(|_| anyhow!("could not decrypt the API key (wrong password?)"))?;
    String::from_utf8(plaintext).context("the decrypted API key is not valid UTF-8")
}

fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| anyhow!("could not derive an encryption key"))?;
    Ok(key)
}

pub fn prompt_password(prompt: &str) -> Result<String> {
    let mut stdout = io::stdout();
    write!(stdout, "{prompt}").context("writing the password prompt")?;
    stdout.flush().context("flushing the password prompt")?;
    enable_raw_mode().context("enabling raw mode for the password prompt")?;
    let result = read_password();
    let _ = disable_raw_mode();
    result
}

fn read_password() -> Result<String> {
    let mut value = String::new();
    loop {
        match event::read().context("reading the password")? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q'))
                {
                    finish_prompt_line();
                    bail!("password entry cancelled");
                }
                match key.code {
                    KeyCode::Enter => {
                        finish_prompt_line();
                        return Ok(value);
                    }
                    KeyCode::Esc => {
                        finish_prompt_line();
                        return Ok(String::new());
                    }
                    KeyCode::Backspace => {
                        if value.pop().is_some() {
                            print!("\u{8} \u{8}");
                            let _ = io::stdout().flush();
                        }
                    }
                    KeyCode::Char(character)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        value.push(character);
                        print!("*");
                        let _ = io::stdout().flush();
                    }
                    _ => {}
                }
            }
            Event::Paste(text) => {
                for character in text.chars().filter(|character| !character.is_control()) {
                    value.push(character);
                    print!("*");
                }
                let _ = io::stdout().flush();
            }
            _ => {}
        }
    }
}

fn finish_prompt_line() {
    print!("\r\n");
    let _ = io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::{open, seal};

    #[test]
    fn sealed_secrets_round_trip() {
        let sealed = seal("correct horse battery staple", "sk-test-123").unwrap();
        let opened = open(
            "correct horse battery staple",
            &sealed.salt,
            &sealed.nonce,
            &sealed.ciphertext,
        )
        .unwrap();
        assert_eq!(opened, "sk-test-123");
    }

    #[test]
    fn a_wrong_password_is_rejected() {
        let sealed = seal("right", "sk-test-123").unwrap();
        assert!(open("wrong", &sealed.salt, &sealed.nonce, &sealed.ciphertext).is_err());
    }
}
