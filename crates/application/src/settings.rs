use crate::App;
use rig_domain::*;

impl App {
    pub async fn farm_settings(&self) -> Result<FarmSettings> {
        Ok(match self.repository.farm_settings().await? {
            // Missing keys (settings added later) fall back to their defaults.
            Some(stored) => serde_json::from_value(stored).unwrap_or_default(),
            None => FarmSettings::default(),
        })
    }
    pub async fn save_farm_settings(
        &self,
        actor: &Actor,
        input: FarmSettings,
    ) -> Result<FarmSettings> {
        input.validate()?;
        let value = serde_json::to_value(&input).map_err(|e| Error::Internal(e.to_string()))?;
        self.repository.save_farm_settings(&value, actor).await?;
        Ok(input)
    }
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
}
