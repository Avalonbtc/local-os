use crate::App;
use rig_domain::*;

impl App {
    /// Settings as clients see them: proxy tokens are only reported as present.
    pub async fn farm_settings(&self) -> Result<FarmSettings> {
        Ok(redacted(self.stored_farm_settings().await?))
    }
    /// Settings as stored, including encrypted proxy tokens (for the status loop).
    pub(crate) async fn stored_farm_settings(&self) -> Result<FarmSettings> {
        Ok(match self.repository.farm_settings().await? {
            // Missing keys (settings added later) fall back to their defaults.
            Some(stored) => serde_json::from_value(stored).unwrap_or_default(),
            None => FarmSettings::default(),
        })
    }
    pub async fn save_farm_settings(
        &self,
        actor: &Actor,
        mut input: FarmSettings,
    ) -> Result<FarmSettings> {
        input.validate()?;
        let previous = self.stored_farm_settings().await?.alerts.proxies;
        for proxy in &mut input.alerts.proxies {
            proxy.pool = proxy.pool.trim().into();
            proxy.api_url = proxy.api_url.trim().into();
            // A typed token replaces the stored one; an empty field keeps it. Whatever a client
            // sends as `token_secret` is ignored.
            proxy.token_secret = match proxy.token.take().filter(|t| !t.trim().is_empty()) {
                Some(token) => Some(self.vault.encrypt(&Credential::Password {
                    password: token.trim().into(),
                    sudo_password: None,
                })?),
                None => previous
                    .iter()
                    .find(|p| ProxyApi::address(&p.pool) == ProxyApi::address(&proxy.pool))
                    .and_then(|p| p.token_secret.clone()),
            };
        }
        let value = serde_json::to_value(&input).map_err(|e| Error::Internal(e.to_string()))?;
        self.repository.save_farm_settings(&value, actor).await?;
        Ok(redacted(input))
    }
    /// The decrypted access token of a configured proxy, if one is stored.
    pub(crate) fn proxy_token(&self, proxy: &ProxyApi) -> Option<String> {
        let secret = proxy.token_secret.as_deref()?;
        match self.vault.decrypt(secret) {
            Ok(Credential::Password { password, .. }) => Some(password),
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(%error, proxy = %proxy.pool, "xmrig-proxy token not decrypted");
                None
            }
        }
    }
}

fn redacted(mut settings: FarmSettings) -> FarmSettings {
    for proxy in &mut settings.alerts.proxies {
        proxy.has_token = proxy.token_secret.take().is_some();
        proxy.token = None;
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_must_be_a_real_tariff() {
        assert_eq!(FarmSettings::default().electricity_price, 0.56);
        for ok in [0.0, 0.35, 100.0] {
            assert!(
                FarmSettings {
                    electricity_price: ok,
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        for bad in [-0.01, 100.01, f64::NAN, f64::INFINITY] {
            assert!(
                FarmSettings {
                    electricity_price: bad,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn stored_settings_missing_a_field_fall_back_to_defaults() {
        let parsed: FarmSettings =
            serde_json::from_value(serde_json::json!({})).unwrap_or_default();
        assert_eq!(parsed, FarmSettings::default());
        // Settings saved before alerts existed keep their price and get default thresholds.
        let parsed: FarmSettings =
            serde_json::from_value(serde_json::json!({"electricity_price": 0.42})).unwrap();
        assert_eq!(parsed.electricity_price, 0.42);
        assert_eq!(parsed.alerts, AlertSettings::default());
        let partial: AlertSettings =
            serde_json::from_value(serde_json::json!({"pool_check": false})).unwrap();
        assert_eq!(partial.local_offline_minutes, 2);
        assert!(!partial.pool_check);
    }

    #[test]
    fn alert_thresholds_are_bounded() {
        let with = |local, pool| FarmSettings {
            alerts: AlertSettings {
                local_offline_minutes: local,
                pool_offline_minutes: pool,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(with(1, 5).validate().is_ok());
        assert!(with(1440, 1440).validate().is_ok());
        for (local, pool) in [(0, 10), (1441, 10), (2, 4), (2, 1441)] {
            assert!(with(local, pool).validate().is_err(), "{local}/{pool}");
        }
    }

    #[test]
    fn proxy_entries_need_an_address_an_http_api_and_no_duplicates() {
        let proxy = |pool: &str, api: &str| ProxyApi {
            pool: pool.into(),
            api_url: api.into(),
            token: None,
            has_token: false,
            token_secret: None,
        };
        let with = |proxies| FarmSettings {
            alerts: AlertSettings {
                proxies,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            with(vec![
                proxy("mac.icuicu.de:6666", "http://mac.icuicu.de:4048"),
                proxy("149.104.4.41:8888", "https://149.104.4.41:4048/"),
            ])
            .validate()
            .is_ok()
        );
        for bad in [
            vec![proxy("", "http://a:1")],
            vec![proxy("a b:1", "http://a:1")],
            vec![proxy("a:1", "ftp://a:1")],
            vec![proxy("a:1", "a:4048")],
            vec![
                proxy("a:1", "http://a:1"),
                proxy("stratum+tcp://A:1", "http://b:1"),
            ],
        ] {
            assert!(with(bad.clone()).validate().is_err(), "{bad:?}");
        }
    }
}
