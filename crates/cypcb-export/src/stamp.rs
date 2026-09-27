//! The moment an export says it was written.
//!
//! Every stamped file carries one [`Stamp`]: a gerber's and a drill file's
//! `TF.CreationDate`, the job file's `CreationDate`, the assembly JSON's
//! `export_date` and an IPC-2581 document's `origination`. A writer takes the
//! stamp as an argument, or from [`crate::ExportJob`], and reads neither the
//! clock nor the environment, so a writer called on its own has no way to
//! fail on the variable. The way in, the CLI, reads the time once with
//! [`export_time`] and stops on a bad value before a file is written.
//!
//! With `SOURCE_DATE_EPOCH` set, the time is the variable's, so two exports of
//! one board are the same bytes. The variable is defined by the
//! `SOURCE_DATE_EPOCH` specification of reproducible-builds.org, revision 1.1
//! of 2017-11-27 (<https://reproducible-builds.org/specs/source-date-epoch/>,
//! read 2026-09-27): "A UNIX timestamp, defined as the number of seconds,
//! excluding leap seconds, since 01 Jan 1970 00:00:00 UTC". Its value "MUST
//! be an ASCII representation of an integer with no fractional component,
//! identical to the output format of `date +%s`", and "If the value is
//! malformed, the build process SHOULD exit with a non-zero error code." A
//! value that is not such an integer is therefore an error that names the
//! variable, never a quiet fall back to the clock. Unset, the time is the
//! clock's.

use std::ffi::OsStr;

use chrono::{DateTime, Utc};

/// The time an export says it was written.
pub type Stamp = DateTime<Utc>;

/// The variable an export takes its time from when it is set.
pub const VARIABLE: &str = "SOURCE_DATE_EPOCH";

/// `SOURCE_DATE_EPOCH` is set to something that is not a time.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{VARIABLE}={value:?} is not a time: {reason}")]
pub struct StampError {
    /// The value as it was set.
    pub value: String,
    /// Why it is not a time.
    pub reason: &'static str,
}

/// The time every file of an export carries: `SOURCE_DATE_EPOCH` when it is
/// set, the clock when it is not.
pub fn export_time() -> Result<Stamp, StampError> {
    Ok(from_value(std::env::var_os(VARIABLE).as_deref())?.unwrap_or_else(Utc::now))
}

/// The time `value` names, or `None` when the variable is not set.
fn from_value(value: Option<&OsStr>) -> Result<Option<DateTime<Utc>>, StampError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let error = |reason| StampError {
        value: value.to_string_lossy().into_owned(),
        reason,
    };
    let text = value.to_str().ok_or_else(|| error("it is not text"))?;
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(error(
            "the specification asks for whole seconds written as `date +%s` writes them",
        ));
    }
    let seconds: i64 = text
        .parse()
        .map_err(|_| error("it does not fit in 64 bits"))?;
    DateTime::from_timestamp(seconds, 0)
        .map(Some)
        .ok_or_else(|| error("it is outside the dates this program can write"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(value: &str) -> Result<Option<DateTime<Utc>>, StampError> {
        from_value(Some(OsStr::new(value)))
    }

    #[test]
    fn a_count_of_seconds_is_that_moment_in_utc() {
        let at = read("1700000000").unwrap().unwrap();
        assert_eq!(
            at.format("%Y-%m-%dT%H:%M:%S%z").to_string(),
            "2023-11-14T22:13:20+0000"
        );
        assert_eq!(read("0").unwrap().unwrap(), DateTime::UNIX_EPOCH);
        assert_eq!(read("-1").unwrap().unwrap().timestamp(), -1);
        assert_eq!(from_value(None), Ok(None));
    }

    #[test]
    fn a_value_that_is_not_whole_seconds_is_an_error_naming_the_variable() {
        for value in [
            "",
            "-",
            "1.5",
            "+1",
            " 1",
            "1\n",
            "1e9",
            "soon",
            "99999999999999999999",
            "9223372036854775807",
        ] {
            let error = read(value).expect_err(value);
            assert!(
                error.to_string().starts_with("SOURCE_DATE_EPOCH="),
                "{error}"
            );
        }
    }
}
