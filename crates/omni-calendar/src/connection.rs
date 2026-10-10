//! The `CalendarConnection` port: whether CalDAV is configured, and with
//! which provider (the email MCP health report).

use omni_runtime::ports::{CalendarConnection, CalendarConnectionStatus};

use crate::caldav::Caldav;

pub struct CaldavConnection {
    caldav: Caldav,
}

impl CaldavConnection {
    pub fn new(caldav: Caldav) -> Self {
        Self { caldav }
    }
}

impl CalendarConnection for CaldavConnection {
    fn status(&self) -> CalendarConnectionStatus {
        let provider = self.caldav.provider();
        CalendarConnectionStatus {
            configured: provider.is_some(),
            provider: provider.map(str::to_owned),
        }
    }
}
