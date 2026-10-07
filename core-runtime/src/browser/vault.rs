use super::*;

impl PageSession {
    /// Saves the current session (cookies) to the vault with the given session ID.
    ///
    /// Gated behind the `session-vault-api` feature (ISSUE-210): `dragon-head-mcp` has no
    /// caller for this today, so it's opt-in rather than shipping as always-compiled,
    /// untested-in-production public surface. Enable the feature (`--features
    /// session-vault-api`) to use it from a library consumer or a future MCP tool/lifecycle
    /// hook — see `docs/session-vault.md`.
    #[cfg(feature = "session-vault-api")]
    pub async fn save_to_vault(&self, session_id: &str) -> Result<()> {
        let cookies = self.inner.get_cookies().context("Failed to get cookies")?;
        let cookie_data: Vec<CookieData> = cookies
            .into_iter()
            .map(|c| CookieData {
                name: c.name,
                value: c.value,
                domain: c.domain,
                path: c.path,
                expires: c.expires,
                size: c.size,
                http_only: c.http_only,
                secure: c.secure,
                session: c.session,
                same_site: c.same_site.map(|s| format!("{:?}", s)),
                priority: format!("{:?}", c.priority),
            })
            .collect();

        let data = SessionData {
            domain: self
                .current_url()
                .context("Failed to resolve current URL while saving session")?,
            cookies: cookie_data,
            tokens: HashMap::new(), // Placeholder for other tokens (e.g. localStorage)
        };

        self.vault.store_session(session_id, &data).await?;
        Ok(())
    }

    /// Loads a session from the vault and restores cookies to the browser.
    ///
    /// Gated behind the `session-vault-api` feature (ISSUE-210) — see
    /// [`save_to_vault`](Self::save_to_vault)'s doc comment for why.
    #[cfg(feature = "session-vault-api")]
    pub async fn load_from_vault(&self, session_id: &str) -> Result<()> {
        use headless_chrome::protocol::cdp::Network::{CookiePriority, CookieSameSite};

        if let Some(data) = self.vault.load_session(session_id).await? {
            let session_url = (!data.domain.is_empty()).then_some(data.domain.clone());
            for cookie in data.cookies {
                let same_site = match cookie.same_site.as_deref() {
                    Some("Strict") => Some(CookieSameSite::Strict),
                    Some("Lax") => Some(CookieSameSite::Lax),
                    Some("None") => Some(CookieSameSite::None),
                    _ => None,
                };

                let priority = match cookie.priority.as_str() {
                    "Low" => Some(CookiePriority::Low),
                    "Medium" => Some(CookiePriority::Medium),
                    "High" => Some(CookiePriority::High),
                    _ => None,
                };
                let expires = if cookie.session {
                    None
                } else {
                    Some(cookie.expires)
                };

                self.inner
                    .call_method(headless_chrome::protocol::cdp::Network::SetCookie {
                        name: cookie.name,
                        value: cookie.value,
                        url: session_url.clone(),
                        domain: Some(cookie.domain),
                        path: Some(cookie.path),
                        secure: Some(cookie.secure),
                        http_only: Some(cookie.http_only),
                        same_site,
                        expires,
                        priority,
                        same_party: None,
                        source_scheme: None,
                        source_port: None,
                        partition_key: None,
                    })
                    .context("Failed to set cookie")?;
            }
        }
        Ok(())
    }
}
