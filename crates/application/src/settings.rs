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
                    electricity_price: ok
                }
                .validate()
                .is_ok()
            );
        }
        for bad in [-0.01, 100.01, f64::NAN, f64::INFINITY] {
            assert!(
                FarmSettings {
                    electricity_price: bad
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
    }
}
