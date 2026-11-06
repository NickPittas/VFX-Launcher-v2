//! GUI-only backup scheduling. Intervals start at terminal completion.
use crate::backup::BackupJob;
use chrono::{DateTime, Local, NaiveDate, TimeZone};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Schedule {
    config: Option<BackupJob>,
    interval_due: Option<Instant>,
    daily_due: Option<DateTime<Local>>,
    last_date: Option<NaiveDate>,
    pending: bool,
}

impl Schedule {
    pub fn update(&mut self, job: &BackupJob) -> Result<(), String> {
        if self.config.as_ref() == Some(job) {
            return Ok(());
        }
        job.validate()?;
        let timing_changed = self.config.as_ref().is_none_or(|previous| {
            previous.schedule_enabled != job.schedule_enabled
                || previous.schedule_mode != job.schedule_mode
                || previous.schedule_interval_minutes != job.schedule_interval_minutes
                || previous.schedule_time != job.schedule_time
        });
        self.config = Some(job.clone());
        if !job.schedule_enabled {
            self.pending = false;
        }
        if timing_changed { self.arm() } else { Ok(()) }
    }

    pub fn due(&mut self, busy: bool) -> bool {
        let Some(job) = &self.config else {
            return false;
        };
        if !job.schedule_enabled {
            self.pending = false;
            return false;
        }
        let reached = if job.schedule_mode == "daily" {
            self.daily_due.is_some_and(|due| Local::now() >= due)
        } else {
            self.interval_due.is_some_and(|due| Instant::now() >= due)
        };
        self.pending |= reached;
        if !self.pending || busy {
            return false;
        }
        self.pending = false;
        if job.schedule_mode == "daily" {
            self.last_date = Some(Local::now().date_naive());
        }
        self.interval_due = None;
        self.daily_due = None;
        true
    }

    pub fn terminal(&mut self) -> Result<(), String> {
        self.arm()
    }
    pub fn retry(&mut self) -> Result<(), String> {
        self.arm()
    }

    pub fn description(&self) -> String {
        if self.config.as_ref().is_none_or(|job| !job.schedule_enabled) {
            return "Manual only".into();
        }
        if self.pending {
            return "Backup due — waiting for active work to finish".into();
        }
        if let Some(due) = self.daily_due {
            return format!("Next backup {}", due.format("%a %d %b %H:%M %Z"));
        }
        if let Some(due) = self.interval_due {
            return format!(
                "Next backup in {} min",
                due.saturating_duration_since(Instant::now())
                    .as_secs()
                    .div_ceil(60)
            );
        }
        "Scheduled backup running".into()
    }

    fn arm(&mut self) -> Result<(), String> {
        self.interval_due = None;
        self.daily_due = None;
        let Some(job) = &self.config else {
            return Ok(());
        };
        if !job.schedule_enabled {
            return Ok(());
        }
        if job.schedule_mode == "interval" {
            self.interval_due = Instant::now().checked_add(Duration::from_secs(
                job.schedule_interval_minutes
                    .checked_mul(60)
                    .ok_or("Backup interval is too large.")?,
            ));
            if self.interval_due.is_none() {
                return Err("Backup interval is too large for the system clock.".into());
            }
        } else {
            let (hour, minute) = job.daily_time()?;
            let now = Local::now();
            let mut date = now.date_naive();
            for _ in 0..370 {
                if self.last_date != Some(date) {
                    let mut wall = date
                        .and_hms_opt(hour, minute, 0)
                        .ok_or("The daily backup time is invalid.")?;
                    // Spring-forward gaps advance to the first valid wall minute.
                    for _ in 0..1441 {
                        if let Some(due) = Local.from_local_datetime(&wall).earliest() {
                            if due > now {
                                self.daily_due = Some(due);
                                return Ok(());
                            }
                            break;
                        }
                        wall = wall
                            .checked_add_signed(chrono::Duration::minutes(1))
                            .ok_or("The daily backup date is out of range.")?;
                    }
                }
                date = date
                    .succ_opt()
                    .ok_or("The next backup date is out of range.")?;
            }
            return Err("The system time zone could not resolve the next daily backup.".into());
        }
        Ok(())
    }
}
