//! Parent password + short-lived admin sessions.

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::RngCore;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A parent session ends after 10 idle minutes, and after 30 minutes at most.
const SESSION_IDLE: Duration = Duration::from_secs(10 * 60);
const SESSION_MAX: Duration = Duration::from_secs(30 * 60);
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);

pub fn hash_password(pw: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default().hash_password(pw.as_bytes(), &salt).expect("argon2 hash").to_string()
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

#[derive(Default)]
pub struct Sessions {
    /// token -> (created, last used)
    tokens: Mutex<HashMap<String, (Instant, Instant)>>,
    failures: Mutex<(u32, Option<Instant>)>,
}

impl Sessions {
    pub fn create(&self) -> String {
        let mut b = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut b);
        let token: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let now = Instant::now();
        let mut t = self.tokens.lock().unwrap();
        t.retain(|_, s| Self::alive(*s, now));
        t.insert(token.clone(), (now, now));
        token
    }

    fn alive((created, used): (Instant, Instant), now: Instant) -> bool {
        now.duration_since(created) < SESSION_MAX && now.duration_since(used) < SESSION_IDLE
    }

    /// Valid token? Using it resets the idle timer, but never extends the
    /// session past its absolute maximum.
    pub fn check(&self, token: &str) -> bool {
        let now = Instant::now();
        let mut t = self.tokens.lock().unwrap();
        match t.get_mut(token) {
            Some(s) if Self::alive(*s, now) => {
                s.1 = now;
                true
            }
            Some(_) => {
                t.remove(token);
                false
            }
            None => false,
        }
    }

    pub fn revoke(&self, token: &str) {
        self.tokens.lock().unwrap().remove(token);
    }

    pub fn revoke_all(&self) {
        self.tokens.lock().unwrap().clear();
    }

    /// Seconds to wait before another attempt is allowed (None = go ahead).
    pub fn locked_for(&self) -> Option<u64> {
        let f = self.failures.lock().unwrap();
        match f.1 {
            Some(until) if f.0 >= MAX_FAILURES && until > Instant::now() => Some((until - Instant::now()).as_secs() + 1),
            _ => None,
        }
    }

    pub fn record_failure(&self) {
        let mut f = self.failures.lock().unwrap();
        if matches!(f.1, Some(until) if until <= Instant::now()) && f.0 >= MAX_FAILURES {
            f.0 = 0;
        }
        f.0 += 1;
        f.1 = Some(Instant::now() + LOCKOUT);
    }

    pub fn record_success(&self) {
        *self.failures.lock().unwrap() = (0, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("hunter2");
        assert!(verify_password("hunter2", &h));
        assert!(!verify_password("hunter3", &h));
    }

    #[test]
    fn lockout_after_failures() {
        let s = Sessions::default();
        for _ in 0..MAX_FAILURES {
            assert!(s.locked_for().is_none());
            s.record_failure();
        }
        assert!(s.locked_for().is_some());
        s.record_success();
        assert!(s.locked_for().is_none());
    }

    #[test]
    fn tokens_work_and_revoke() {
        let s = Sessions::default();
        let t = s.create();
        assert!(s.check(&t));
        s.revoke(&t);
        assert!(!s.check(&t));
    }
}
