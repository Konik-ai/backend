use chrono::prelude::Utc;
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, TransactionTrait, QueryOrder};
use loco_rs::prelude::*;
pub use super::_entities::devices::{self, ActiveModel, Entity, Model as DM, Column};
use crate::controllers::v2::DeviceRegistrationParams;
use sea_orm::{DbBackend, Statement};


#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    // extend activemodel below (keep comment for generators)
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        if insert {
            this.created_at = ActiveValue::Set(Utc::now().naive_utc());
            this.updated_at = ActiveValue::Set(Utc::now().naive_utc());
            Ok(this)
        } else {
            // update time
            this.updated_at = ActiveValue::Set(Utc::now().naive_utc());
            Ok(this)
        }
    }
}

impl DM {
    /// Aggregate devices per day (UTC) for all time.
    /// Returns two series:
    /// - daily registrations by created_at day (count of devices created each day)
    /// - daily active pings by last_athena_ping day (count of pings each day)
    pub async fn daily_devices_all_time(
        db: &DatabaseConnection,
    ) -> Result<(Vec<(i64, i64)>, Vec<(i64, i64)>), DbErr> {
        // Registrations per day
        let sql_reg = r#"
            SELECT 
                (EXTRACT(EPOCH FROM created_at)::bigint * 1000 / 86400000) * 86400000 as day_ms,
                COUNT(*) as cnt
            FROM devices
            GROUP BY day_ms
            ORDER BY day_ms
        "#;
        let stmt_reg = Statement::from_string(DbBackend::Postgres, sql_reg.to_string());
        let reg_rows_raw = db.query_all(stmt_reg).await?;
        let mut reg_rows: Vec<(i64, i64)> = Vec::with_capacity(reg_rows_raw.len());
        for row in reg_rows_raw {
            let day_ms: i64 = row.try_get("", "day_ms")?;
            let cnt: i64 = row.try_get("", "cnt")?;
            reg_rows.push((day_ms, cnt));
        }

        // Active pings per day
        let sql_active = r#"
            SELECT 
                (last_athena_ping / 86400000) * 86400000 as day_ms,
                COUNT(*) as cnt
            FROM devices
            WHERE last_athena_ping > 0
            GROUP BY day_ms
            ORDER BY day_ms
        "#;
        let stmt_active = Statement::from_string(DbBackend::Postgres, sql_active.to_string());
        let active_rows_raw = db.query_all(stmt_active).await?;
        let mut active_rows: Vec<(i64, i64)> = Vec::with_capacity(active_rows_raw.len());
        for row in active_rows_raw {
            let day_ms: i64 = row.try_get("", "day_ms")?;
            let cnt: i64 = row.try_get("", "cnt")?;
            active_rows.push((day_ms, cnt));
        }

        Ok((reg_rows, active_rows))
    }

    pub async fn register_device(
        db: &DatabaseConnection,
        params: DeviceRegistrationParams,
        dongle_id: &String,
    ) -> ModelResult<()> {
        // Check if the device is registered already
        match DM::find_device(db, dongle_id).await {
            Ok(_) => Ok(()),
            Err(_e) => {
                // Add device to db
                let txn = db.begin().await?;
                let device = DM {
                    dongle_id: dongle_id.clone(),
                    public_key: params.public_key,
                    imei: params.imei,
                    imei2: params.imei2,
                    serial: params.serial,
                    uploads_allowed: true,
                    prime: true,
                    prime_type: 2,
                    alias: "Please set to discord username".to_string(),
                    firehose: true,
                    ..Default::default()
                };
                device.into_active_model().insert(&txn).await?;
                txn.commit().await?;
                Ok(())
            },
        }
    }
    /// Find all devices associated with a user
    /// 
    /// 
    /// Returns a list of devices associated with the user.
    /// Can be empty if the user has no devices
    pub async fn find_user_devices(
        db: &DatabaseConnection,
        user_id: i32,
    ) -> Vec<DM> {
        Entity::find()
            .filter(Column::OwnerId.eq(user_id))
            .order_by_desc(Column::Online)
            .order_by_desc(Column::LastAthenaPing)
            .all(db)
            .await
            .expect("Database query failed")
    }

    pub async fn find_user_device(
        db: &DatabaseConnection,
        user_id: i32,
        dongle_id: &str
    ) -> Result<Option<DM>, DbErr> {
        Entity::find()
            .filter(Column::OwnerId.eq(user_id))
            .filter(Column::DongleId.eq(dongle_id))
            .one(db)
            .await
    }

    pub async fn ensure_user_device(
        db: &DatabaseConnection,
        user_id: i32,
        dongle_id: &str
    ) -> Result<DM, DbErr> {
        Entity::find()
            .filter(Column::OwnerId.eq(user_id))
            .filter(Column::DongleId.eq(dongle_id))
            .one(db)
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("Device not found for that owner".to_string()))
    }

    pub async fn find_all_devices(
        db: &DatabaseConnection,
    ) -> Vec<DM> {
        Entity::find()
            .order_by_desc(Column::Online)
            .order_by_desc(Column::LastAthenaPing)
            .all(db)
            .await
            .expect("Database query failed")
    }

    pub async fn find_device(
        db: &DatabaseConnection,
        dongle_id: &str,
    ) -> ModelResult<DM> {
        let device = Entity::find()
            .filter(Column::DongleId.eq(dongle_id))
            .one(db)
            .await?;
        device.ok_or_else(|| ModelError::EntityNotFound)
    }

    pub async fn reset_online(
        db: &DatabaseConnection,
    ) -> Result<(), DbErr> {
        // Update all devices to set `Online` to `false`
        Entity::update_many()
            .col_expr(Column::Online, Expr::value(false))
            .exec(db)
            .await?;
            
        Ok(())
    }

    pub async fn get_locations(
        db: &DatabaseConnection,
        dongle_id: &str,
    ) -> ModelResult<Option<serde_json::Value>> {
        let device = Entity::find()
            .filter(Column::DongleId.eq(dongle_id))
            .one(db)
            .await?;
        let device = device.ok_or(ModelError::EntityNotFound)?;
        // Return the optional JSON data stored in the locations field
        Ok(device.locations)
    }

    pub async fn get_registered_devices(
        db: &DatabaseConnection,
        online_now: Option<bool>,
        registered_after: Option<DateTime>,
        last_ping_time_after: Option<u64>,
    ) -> Result<u64, DbErr> {
        let mut q = Entity::find();
        if let Some(online_now) = online_now {
            q = q.filter(Column::Online.gte(online_now))
        }
        if let Some(registered_after) = registered_after {
            q = q.filter(Column::CreatedAt.gte(registered_after))
        }
        if let Some(last_ping_time_after) = last_ping_time_after {
            q = q.filter(Column::LastAthenaPing.gte(last_ping_time_after))
        }
        q.count(db).await
    }

    /// Returns daily device counts since a given unix ms (based on created_at as registration and last_athena_ping as activity)
    /// - registrations: number of devices with created_at on each day
    /// - active: number of devices that pinged on each day (last_athena_ping in that day)
    pub async fn daily_devices_since(
        db: &DatabaseConnection,
        start_ms: i64,
    ) -> Result<(Vec<(String, i64)>, Vec<(String, i64)>), DbErr> {
        // registrations per day
        let sql_reg = r#"
            SELECT 
                to_char(timezone('UTC', created_at), 'YYYY-MM-DD') AS day,
                COUNT(*)::bigint AS cnt
            FROM devices
            WHERE EXTRACT(EPOCH FROM created_at) * 1000 >= $1
            GROUP BY day
            ORDER BY day
        "#;
        let stmt_reg = Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql_reg,
            vec![sea_orm::Value::BigInt(Some(start_ms))],
        );
        let reg_rows = db.query_all(stmt_reg).await?;
        let mut regs: Vec<(String, i64)> = Vec::with_capacity(reg_rows.len());
        for row in reg_rows {
            let day: String = row.try_get("", "day")?;
            let cnt: i64 = row.try_get("", "cnt")?;
            regs.push((day, cnt));
        }

        // active pings per day - count devices that had a ping in that day
        let sql_active = r#"
            SELECT 
                to_char(timezone('UTC', to_timestamp(last_athena_ping/1000)), 'YYYY-MM-DD') AS day,
                COUNT(*)::bigint AS cnt
            FROM devices
            WHERE last_athena_ping >= $1
            GROUP BY day
            ORDER BY day
        "#;
        let stmt_active = Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql_active,
            vec![sea_orm::Value::BigInt(Some(start_ms))],
        );
        let active_rows = db.query_all(stmt_active).await?;
        let mut actives: Vec<(String, i64)> = Vec::with_capacity(active_rows.len());
        for row in active_rows {
            let day: String = row.try_get("", "day")?;
            let cnt: i64 = row.try_get("", "cnt")?;
            actives.push((day, cnt));
        }

        Ok((regs, actives))
    }
}