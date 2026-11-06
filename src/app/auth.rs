//! Session policy and the authentication boundary: lock state, login/logout,
//! freeze, and the idle/maximum session expiry.
use super::{totp, App, Session};
use crate::{crypto::Key, engine::Engine, journal, model::GraphSlice, store::Payload};
use anyhow::{bail, Result};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Locked,
    Developer,
    Frozen,
}
#[derive(Clone, Copy)]
pub struct SessionPolicy {
    pub idle: Duration,
    pub maximum: Duration,
}
impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(1800),
            maximum: Duration::from_secs(8 * 3600),
        }
    }
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

impl App {
    pub fn tick(&mut self) {
        if self.session.as_ref().is_some_and(|s| {
            s.last.elapsed() >= self.policy.idle || s.started.elapsed() >= self.policy.maximum
        }) {
            self.session = None;
        }
    }
    pub fn mode(&mut self) -> Mode {
        self.tick();
        self.session
            .as_ref()
            .map(|s| {
                if s.frozen {
                    Mode::Frozen
                } else {
                    Mode::Developer
                }
            })
            .unwrap_or(Mode::Locked)
    }
    pub fn login(&mut self, user: &str, password: &str, code: Option<&str>) -> Result<()> {
        // A failed reauthentication cannot leave an old developer session active.
        self.logout();
        let time = now()?;
        self.store.check_throttle(time)?;
        let attempt = (|| -> Result<(Payload, Key)> {
            let (mut payload, key) = self.store.unlock(user, password)?;
            if let Some(secret) = &payload.totp {
                let otp = totp(secret)?;
                let token = code.unwrap_or("");
                if token.len() != 6 || !token.bytes().all(|b| b.is_ascii_digit()) {
                    bail!("Anmeldung fehlgeschlagen");
                }
                // Check skew explicitly and persist the consumed counter before granting access.
                let step = time / 30;
                let mut strict = otp;
                strict.skew = 0;
                let matched = [step.saturating_sub(1), step, step + 1]
                    .into_iter()
                    .find(|s| {
                        payload.last_totp.is_none_or(|last| *s > last)
                            && strict.check(token, *s * 30)
                    });
                payload.last_totp =
                    Some(matched.ok_or_else(|| anyhow::anyhow!("Anmeldung fehlgeschlagen"))?);
            }
            Ok((payload, key))
        })();
        let (payload, key) = match attempt {
            Ok(ok) => ok,
            Err(_) => {
                self.store.failure(now()?)?;
                bail!("Anmeldung fehlgeschlagen");
            }
        };
        self.store.authenticated(&payload, &key)?;
        // Crash recovery: journal entries beyond the committed confirmation
        // seq never belonged to a persisted state and are discarded here.
        let kap = journal::kap(&payload.state);
        let (session_journal, verworfen) =
            self.store.journal_sync(&key, payload.op_journal_seq, kap)?;
        self.verworfen_ungemeldet = self.verworfen_ungemeldet.saturating_add(verworfen);
        let graph_cache = if payload.state.neuronen.is_empty() {
            GraphSlice::default()
        } else {
            GraphSlice {
                nodes: payload.state.neuronen.clone(),
                edges: payload.state.kanten.clone(),
            }
        };
        let active = Engine::active_state(&payload.state, &graph_cache);
        let engine = Engine::new(active, payload.actions.clone())?;
        self.session = Some(Session {
            engine,
            graph_cache,
            prefetch_status: None,
            key: Some(key),
            read_key: None,
            feedback_target: payload.state.extra.get("feedback_target").cloned(),
            payload,
            started: Instant::now(),
            last: Instant::now(),
            frozen: false,
            journal: session_journal,
        });
        Ok(())
    }
    pub fn logout(&mut self) {
        self.session = None;
    }
    pub fn freeze(&mut self) -> Result<()> {
        self.tick();
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Anmeldung erforderlich"))?;
        if !s.frozen {
            s.read_key = s.key.take();
        }
        s.frozen = true;
        if let Some(secret) = &mut s.payload.totp {
            secret.zeroize();
        }
        s.payload.totp = None;
        s.last = Instant::now();
        Ok(())
    }
}
