#![allow(clippy::unused_async)]
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use sysinfo::{
    Disks
};
use crate::{
    models::{
        devices::DM,
        routes::RM,
    }
};
use crate::{
    views
};
use chrono::TimeZone;
#[derive(Serialize, Deserialize)]
pub struct TimeSeriesPoint {
    pub date: String,
    pub miles: i64,
}
#[derive(Serialize, Deserialize)]
pub struct CpuUsage {
    pub core: u8,
    pub usage: f32,
}
#[derive(Serialize, Deserialize)]
pub struct DiskSpace {
    pub total: u64,
    pub used: u64,
    pub free: u64,
}
#[derive(Serialize, Deserialize)]
pub struct Active {
    pub daily: u64,
    pub weekly: u64,
    pub monthly: u64,
    pub quarterly: u64,
}

#[derive(Serialize, Deserialize)]
pub struct Devices {
    pub online: u64,
    pub total: u64,
    pub active: Active,
}
#[derive(Serialize, Deserialize)]
pub struct Network {
    pub current_upload: f32,
    pub current_download: f32,
    pub total_upload: f32,
    pub total_download: f32,
}
#[derive(Serialize, Deserialize)]
pub struct DriveStats {
    pub total_miles: i32,
    pub total_drives: u64,
}

#[derive(Serialize, Deserialize)]
pub struct ServerUsage {
    pub time: String,
    pub disk_usage: Vec<DiskSpace>,
    //users: u64,
    pub devices: Devices,
    pub drive_stats: DriveStats,
    // Charts data expected by the template
    pub miles_over_time: Vec<TimeSeriesPoint>,
    pub daily_miles_over_time: Vec<TimeSeriesPoint>,
    pub devices_over_time: Vec<TimeSeriesPoint>,
    pub daily_devices_over_time: Vec<TimeSeriesPoint>,
}


async fn get_disk_usage() -> Vec<DiskSpace> {
    let disks = Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|disk| disk.name() == "/dev/md0")
        .map(|disk| DiskSpace {
            total: disk.total_space(),
            used: disk.total_space() - disk.available_space(),
            free: disk.available_space(),
        })
        .collect()
}

pub async fn get_server_usage(
    ViewEngine(view): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
) -> Result<impl IntoResponse> {
    let utc_time_now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u64;
    let one_day_ago_millis = utc_time_now_millis - std::time::Duration::from_secs(24 * 60 * 60).as_secs() as u64;
    let one_week_ago_millis = utc_time_now_millis - std::time::Duration::from_secs(7 * 24 * 60 * 60).as_secs() as u64;
    let one_month_ago_millis = utc_time_now_millis - std::time::Duration::from_secs(30 * 24 * 60 * 60).as_secs() as u64;
    let three_months_ago_millis = utc_time_now_millis - std::time::Duration::from_secs(90 * 24 * 60 * 60).as_secs() as u64;

    // All-time daily miles (sum of route length per UTC day)
    let daily_miles_rows = RM::daily_miles_all_time(&ctx.db).await.unwrap_or_default();
    let daily_miles_points: Vec<TimeSeriesPoint> = daily_miles_rows
        .iter()
        .map(|(day_ms, miles)| {
            let date = chrono::Utc
                .timestamp_millis_opt(*day_ms)
                .single()
                .unwrap_or_else(|| chrono::Utc.timestamp_millis_opt(0).unwrap())
                .format("%Y-%m-%d")
                .to_string();
            TimeSeriesPoint { date, miles: *miles as i64 }
        })
        .collect();
    let mut cumulative_miles_points: Vec<TimeSeriesPoint> = Vec::with_capacity(daily_miles_points.len());
    let mut acc: i64 = 0;
    for p in &daily_miles_points {
        acc += p.miles;
        cumulative_miles_points.push(TimeSeriesPoint { date: p.date.clone(), miles: acc });
    }

    // Fetch all-time device series
    // registrations by created_at and activity by last_athena_ping
    let (daily_reg_rows, daily_active_rows) = DM::daily_devices_all_time(&ctx.db).await.unwrap_or_default();
    // We'll display "devices over time" as cumulative registrations, and daily_devices as daily actives
    let daily_devices_points: Vec<TimeSeriesPoint> = daily_active_rows
        .iter()
        .map(|(day_ms, cnt)| {
            let date = chrono::Utc
                .timestamp_millis_opt(*day_ms)
                .single()
                .unwrap_or_else(|| chrono::Utc.timestamp_millis_opt(0).unwrap())
                .format("%Y-%m-%d")
                .to_string();
            TimeSeriesPoint { date, miles: *cnt as i64 }
        })
        .collect();
    let mut cumulative_devices_points: Vec<TimeSeriesPoint> = Vec::with_capacity(daily_reg_rows.len());
    let mut acc_dev: i64 = 0;
    for (day_ms, cnt) in &daily_reg_rows {
        acc_dev += *cnt as i64;
        let date = chrono::Utc
            .timestamp_millis_opt(*day_ms)
            .single()
            .unwrap_or_else(|| chrono::Utc.timestamp_millis_opt(0).unwrap())
            .format("%Y-%m-%d")
            .to_string();
        cumulative_devices_points.push(TimeSeriesPoint { date, miles: acc_dev });
    }
    
    
    views::route::server_usage(
        view,
        ServerUsage {
            time: chrono::Utc::now().to_rfc3339(),
            disk_usage:  get_disk_usage().await,
            devices: Devices {
                total: DM::get_registered_devices(&ctx.db,None, None, None).await?,
                online: DM::get_registered_devices(&ctx.db,Some(true), None, None).await?,
                active: Active {
                    daily: DM::get_registered_devices(&ctx.db,None, None, Some(one_day_ago_millis)).await?,
                    weekly: DM::get_registered_devices(&ctx.db,None, None, Some(one_week_ago_millis)).await?,
                    monthly: DM::get_registered_devices(&ctx.db,None, None, Some(one_month_ago_millis)).await?,
                    quarterly: DM::get_registered_devices(&ctx.db,None, None, Some(three_months_ago_millis)).await?

                },
            },
            drive_stats: DriveStats { 
                total_miles: RM::get_miles(&ctx.db).await? as i32, 
                total_drives: RM::get_drive_count(&ctx.db).await?
            },
            // Provide time series for charts
            miles_over_time: cumulative_miles_points,
            daily_miles_over_time: daily_miles_points,
            devices_over_time: cumulative_devices_points,
            daily_devices_over_time: daily_devices_points,
        },
    )
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("stats")
        .add("/usage", get(get_server_usage))
}
