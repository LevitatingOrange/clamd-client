use nom::branch::alt;
use nom::bytes::complete::{tag, take_till, take_until};
use nom::character::complete::char;
use nom::combinator::{cond, eof, map_res};
use nom::sequence::{terminated, tuple};
use nom::{Finish, IResult, Parser};

use crate::error::{ClamdError, Result};

#[derive(Debug, PartialEq)]
pub struct ScanResult {
    /// used when scanning multiple entities in a single session
    pub scan_id: u64,
    /// the scanned entity, can be a file path, the word "stream"
    pub scanned_entity: String,
    pub condition: ScanCondition,
}

#[derive(Debug, PartialEq)]
pub enum ScanCondition {
    Malignant(String),
    Benign,
}

enum ScanAnswer<'a> {
    Ok,
    Found(&'a str),
    Error(&'a str),
}

fn parse_scan_result(input: &str) -> IResult<&str, ScanAnswer> {
    alt((
        terminated(take_until("OK"), tag("OK")).map(|_| ScanAnswer::Ok),
        terminated(take_until("FOUND"), tag("FOUND")).map(ScanAnswer::Found),
        terminated(take_until("ERROR"), tag("ERROR")).map(ScanAnswer::Error),
    ))(input)
}

fn parse_entity(input: &str) -> IResult<&str, &str> {
    terminated(take_till(|c| c == ':'), char(':'))(input)
}

fn parse_session_id(input: &str) -> IResult<&str, u64> {
    map_res(terminated(take_till(|c| c == ':'), char(':')), |s| {
        u64::from_str_radix(s, 10)
    })(input)
}

impl ScanResult {
    pub(crate) fn parse(input: &str, in_session: bool) -> Result<Self> {
        let parser_result: IResult<&str, (Option<u64>, &str, ScanAnswer)> = terminated(
            tuple((
                cond(in_session, parse_session_id),
                parse_entity,
                parse_scan_result,
            )),
            eof,
        )(input);

        let (_, (session_id, scanned_entity, answer)) = parser_result
            .finish()
            .map_err(|err: nom::error::Error<&str>| ClamdError::InvalidResponse(err.to_string()))?;

        let condition = match answer {
            ScanAnswer::Ok => Ok(ScanCondition::Benign),
            ScanAnswer::Found(msg) => Ok(ScanCondition::Malignant(msg.trim().to_owned())),
            ScanAnswer::Error(msg) => Err(ClamdError::ScanError(
                scanned_entity.trim().to_owned(),
                msg.trim().to_owned(),
            )),
        }?;
        Ok(ScanResult {
            scan_id: session_id.unwrap_or(0),
            scanned_entity: scanned_entity.trim().to_owned(),
            condition,
        })
    }
}

#[cfg(test)]
mod test {
    use crate::ClamdError;

    use super::{ScanCondition, ScanResult};

    const STREAM_FOUND: &str = "stream: Win.Test.EICAR_HDB-1 FOUND";
    const STREAM_FOUND_SESSION: &str = "053: stream: Win.Test.EICAR_HDB-1 FOUND";
    const STREAM_ERROR: &str = "stream: we got error ERROR";
    const STREAM_OK: &str = "stream: Win.Test.EICAR_HDB-1 OK";
    const FILE_FOUND: &str = "/home/foo/clamd-client/eicarcom2.zip: Win.Test.EICAR_HDB-1 FOUND";
    const INVALID: &str = "/home/foo/clamd-client/eicarcom2.zip: Win.Test.EICAR_HDB-aufnt ";

    #[test]
    fn parse_result() -> eyre::Result<()> {
        assert_eq!(
            ScanResult::parse(STREAM_FOUND, false)?,
            ScanResult {
                scan_id: 0,
                scanned_entity: "stream".to_owned(),
                condition: ScanCondition::Malignant("Win.Test.EICAR_HDB-1".to_owned())
            }
        );
        assert_eq!(
            ScanResult::parse(STREAM_FOUND_SESSION, true)?,
            ScanResult {
                scan_id: 53,
                scanned_entity: "stream".to_owned(),
                condition: ScanCondition::Malignant("Win.Test.EICAR_HDB-1".to_owned())
            }
        );
        match ScanResult::parse(STREAM_ERROR, false) {
            Err(ClamdError::ScanError(ent, msg)) => {
                assert_eq!(ent, "stream");
                assert_eq!(msg, "we got error");
            }
            res => panic!("Should err but got {:?}", res),
        }

        match ScanResult::parse(INVALID, false) {
            Err(ClamdError::InvalidResponse(msg)) => {
                assert_eq!(msg, "error TakeUntil at:  Win.Test.EICAR_HDB-aufnt ");
            }
            res => panic!("Should err but got {:?}", res),
        }

        assert_eq!(
            ScanResult::parse(STREAM_OK, false)?,
            ScanResult {
                scan_id: 0,
                scanned_entity: "stream".to_owned(),
                condition: ScanCondition::Benign
            }
        );

        assert_eq!(
            ScanResult::parse(FILE_FOUND, false)?,
            ScanResult {
                scan_id: 0,
                scanned_entity: "/home/foo/clamd-client/eicarcom2.zip".to_owned(),
                condition: ScanCondition::Malignant("Win.Test.EICAR_HDB-1".to_owned())
            }
        );

        Ok(())
    }
}
