use super::*;
use serde_json::json;
use uuid::Uuid;

const TOKEN: &str = "SELECT to_jsonb(t) FROM enrollment_tokens t";

#[async_trait::async_trait]
impl EnrollmentRepository for PgStore {
    async fn enrollment_tokens(&self) -> Result<Vec<EnrollmentToken>> {
        sqlx::query_scalar::<_, Value>(&format!("{TOKEN} ORDER BY created_at DESC"))
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(decode)
            .collect()
    }
    async fn enrollment_token(&self, id: Uuid) -> Result<EnrollmentToken> {
        decode(
            sqlx::query_scalar::<_, Value>(&format!("{TOKEN} WHERE id=$1"))
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(db)?,
        )
    }
    async fn create_enrollment_token(
        &self,
        id: Uuid,
        input: &EnrollmentTokenInput,
        actor: &Actor,
    ) -> Result<EnrollmentToken> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("INSERT INTO enrollment_tokens(id,name,\"group\",tags,expires_at,max_uses,created_by) VALUES($1,$2,$3,$4,now()+make_interval(hours=>$5),$6,$7)")
            .bind(id)
            .bind(&input.name)
            .bind(&input.group)
            .bind(json!(input.tags))
            .bind(input.expires_in_hours.map(|h| h as i32))
            .bind(input.max_uses)
            .bind(&actor.name)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        audit_tx(
            &mut tx,
            actor,
            "fleet.enrollment_token_create",
            &id.to_string(),
            json!({"name":input.name,"expires_in_hours":input.expires_in_hours,"max_uses":input.max_uses}),
        )
        .await?;
        tx.commit().await.map_err(db)?;
        self.enrollment_token(id).await
    }
    async fn revoke_enrollment_token(&self, id: Uuid, actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let changed = sqlx::query(
            "UPDATE enrollment_tokens SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?
        .rows_affected();
        if changed == 0 {
            return Err(Error::NotFound);
        }
        audit_tx(
            &mut tx,
            actor,
            "fleet.enrollment_token_revoke",
            &id.to_string(),
            json!({}),
        )
        .await?;
        tx.commit().await.map_err(db)
    }
    async fn enroll_machine(
        &self,
        token: Uuid,
        machine: &NewAgentMachine,
    ) -> Result<(Machine, bool)> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        // Row lock: concurrent installs cannot exceed max_uses.
        let usable: Option<String> = sqlx::query_scalar(
            "SELECT name FROM enrollment_tokens WHERE id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now()) AND (max_uses IS NULL OR uses<max_uses) FOR UPDATE",
        )
        .bind(token)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        let Some(token_name) = usable else {
            return Err(Error::Unauthorized);
        };
        let (group, tags): (String, Value) =
            sqlx::query_as("SELECT \"group\",tags FROM enrollment_tokens WHERE id=$1")
                .bind(token)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
        sqlx::query("UPDATE enrollment_tokens SET uses=uses+1 WHERE id=$1")
            .bind(token)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM machines WHERE fingerprint=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(&machine.fingerprint)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        let (id, created) = match existing {
            Some(id) => {
                // A reinstall: same rig, possibly a new address.
                sqlx::query("UPDATE machines SET host=$2,port=22,username='root',host_key='agent',transport='agent',ssh_secret=$3 WHERE id=$1")
                    .bind(id)
                    .bind(&machine.host)
                    .bind(&machine.ssh_secret)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                (id, false)
            }
            None => {
                // Names stay unique across deleted machines too: take the first free suffix.
                let taken: Vec<String> = sqlx::query_scalar(
                    "SELECT name FROM machines WHERE name=$1 OR name LIKE $1 || '-%'",
                )
                .bind(&machine.name)
                .fetch_all(&mut *tx)
                .await
                .map_err(db)?;
                let name = std::iter::once(machine.name.clone())
                    .chain((2..).map(|n| format!("{}-{n}", machine.name)))
                    .find(|candidate| !taken.contains(candidate))
                    .expect("an unused suffix exists");
                let id = Uuid::new_v4();
                sqlx::query("INSERT INTO machines(id,name,host,port,username,host_key,\"group\",tags,policy,ssh_secret,transport,fingerprint) VALUES($1,$2,$3,22,'root','agent',$4,$5,'{}',$6,'agent',$7)")
                    .bind(id)
                    .bind(&name)
                    .bind(&machine.host)
                    .bind(if machine.group.is_empty() { &group } else { &machine.group })
                    .bind(if machine.tags.is_empty() { tags } else { json!(machine.tags) })
                    .bind(&machine.ssh_secret)
                    .bind(&machine.fingerprint)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                (id, true)
            }
        };
        sqlx::query(
            "INSERT INTO audit_events(actor,action,target,detail) VALUES($1,'fleet.enroll',$2,$3)",
        )
        .bind(format!("安装密钥：{token_name}"))
        .bind(id.to_string())
        .bind(json!({"token_id":token,"created":created,"host":machine.host}))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok((self.machine(id).await?, created))
    }
}
