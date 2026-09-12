//! OFX/QFX structural parsing (Import Engine v2 batch 3, specs/phase-010).
//! This module's job ends at producing a structured, MIDAS-agnostic
//! representation of what the file actually contains - source accounts,
//! their raw transaction fields, and statement balances. Turning that into
//! `NormalizedCandidateTransaction`s (sign convention, transaction-type
//! inference, rule/payee resolution) is `import.rs`'s job, exactly
//! mirroring how this module's CSV counterpart (`read_csv`/`detect_mapping`
//! in `import.rs`) stays separate from `build_staging_rows`.
//!
//! OFX ships in two eras that this module normalizes into one shape before
//! extraction: OFX 1.x is SGML, where a leaf tag's closing tag is implicit
//! (`<TRNAMT>-4.50` runs until the next `<`, with no `</TRNAMT>`) - not
//! valid XML on its own. OFX 2.x (and QFX, which uses the 1.x SGML style
//! with a few Intuit-specific tags this module ignores) is well-formed
//! XML. `sgml_to_xml` synthesizes the missing closing tags so the same
//! `quick_xml` event walker (`extract`) handles both uniformly - no
//! separate SGML-specific extraction path.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Bank,
    CreditCard,
}

/// A source account as described by the statement itself - not yet
/// associated with any MIDAS account (see `ofx_accounts.rs` for that).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceAccount {
    pub bank_id: Option<String>,
    pub source_account_id: String,
    pub source_account_type: Option<String>,
    pub kind: StatementKind,
    pub currency: Option<String>,
}

/// The raw fields of one `<STMTTRN>` - deliberately only the ones product
/// handoff section 8 names as actually used, not everything the OFX
/// specification defines.
#[derive(Debug, Clone, Default)]
pub struct OfxTransaction {
    pub fitid: Option<String>,
    pub trntype: Option<String>,
    pub dtposted: Option<String>,
    pub dtuser: Option<String>,
    pub trnamt: Option<String>,
    pub name: Option<String>,
    pub memo: Option<String>,
    pub checknum: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StatementBalance {
    pub amount: String,
    pub as_of_date: String,
}

#[derive(Debug, Clone)]
pub struct OfxStatement {
    pub account: SourceAccount,
    pub transactions: Vec<OfxTransaction>,
    pub ledger_balance: Option<StatementBalance>,
    /// The statement period (`BANKTRANLIST`'s `DTSTART`/`DTEND`) - reconciliation
    /// (specs/phase-011) uses this as a prefill suggestion, never an
    /// automatic decision (the owner still confirms). `None` when either
    /// tag is absent, which real-world OFX occasionally does.
    pub statement_start_date: Option<String>,
    pub statement_end_date: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedOfx {
    pub statements: Vec<OfxStatement>,
    /// Detected but never extracted (product handoff section 6: investment
    /// statements are out of scope this batch and must never be partially
    /// imported as normal transactions).
    pub has_investment_statement: bool,
}

#[derive(Debug)]
pub enum OfxError {
    Malformed(String),
    InvestmentOnly,
    NoSupportedStatements,
}

impl fmt::Display for OfxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OfxError::Malformed(msg) => write!(f, "Could not read this OFX/QFX file: {msg}"),
            OfxError::InvestmentOnly => write!(
                f,
                "This file contains an investment/brokerage statement, which MIDAS does not support importing yet."
            ),
            OfxError::NoSupportedStatements => write!(
                f,
                "No checking, savings, or credit-card statement was found in this file."
            ),
        }
    }
}

/// Parses OFX 1.x SGML or OFX 2.x/QFX XML into a structural representation.
/// Never partially imports an investment statement - if one is present
/// alongside supported statements, it is skipped and `has_investment_statement`
/// is set; if it is the *only* content, this returns `InvestmentOnly`.
pub fn parse(content: &str) -> Result<ParsedOfx, OfxError> {
    let xml = normalize_to_xml(content)?;
    let parsed = extract(&xml)?;
    if parsed.statements.is_empty() {
        if parsed.has_investment_statement {
            return Err(OfxError::InvestmentOnly);
        }
        return Err(OfxError::NoSupportedStatements);
    }
    Ok(parsed)
}

fn normalize_to_xml(content: &str) -> Result<String, OfxError> {
    let looks_like_xml =
        content.trim_start().starts_with("<?xml") || content.contains("OFXHEADER=\"200\"");
    let body_start = content
        .find('<')
        .ok_or_else(|| OfxError::Malformed("no OFX content found in this file".to_string()))?;

    if looks_like_xml {
        return Ok(content[body_start..].to_string());
    }
    sgml_to_xml(&content[body_start..])
}

/// Converts OFX 1.x SGML into well-formed XML by synthesizing a closing tag
/// immediately after any leaf tag's inline value - the one structural fact
/// that separates OFX SGML from XML. A tag immediately followed by another
/// tag (only whitespace between) is a container and is left as-is; its own
/// explicit closing tag already exists later in the SGML, same as XML.
fn sgml_to_xml(sgml: &str) -> Result<String, OfxError> {
    let mut output = String::with_capacity(sgml.len() + sgml.len() / 4);
    let len = sgml.len();
    let mut i = 0;

    while i < len {
        if !sgml.is_char_boundary(i) || sgml.as_bytes()[i] != b'<' {
            i += 1;
            continue;
        }
        let tag_end = sgml[i..]
            .find('>')
            .map(|p| p + i)
            .ok_or_else(|| OfxError::Malformed("an unterminated tag".to_string()))?;
        let tag_content = &sgml[i + 1..tag_end];
        output.push('<');
        output.push_str(tag_content);
        output.push('>');

        if tag_content.starts_with('/') || tag_content.ends_with('/') || tag_content.is_empty() {
            i = tag_end + 1;
            continue;
        }

        let after = tag_end + 1;
        let next_lt = sgml[after..].find('<').map(|p| p + after).unwrap_or(len);
        let value = sgml[after..next_lt].trim();
        if !value.is_empty() {
            output.push_str(&xml_escape(value));
            output.push_str("</");
            output.push_str(tag_content);
            output.push('>');
        }
        i = next_lt;
    }

    Ok(output)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Default)]
struct AccountBuilder {
    bank_id: Option<String>,
    source_account_id: Option<String>,
    source_account_type: Option<String>,
}

#[derive(Default)]
struct BalanceBuilder {
    amount: Option<String>,
    as_of_date: Option<String>,
}

fn parent_is(stack: &[String], name: &str) -> bool {
    stack.len() >= 2 && stack[stack.len() - 2] == name
}

/// Walks the (now-uniform) XML with `quick_xml`, tracking a simple tag-name
/// stack rather than building a full DOM - OFX's structure is fixed and
/// shallow enough that "is my parent tag X" is all any field needs to
/// disambiguate context (e.g. `ACCTID` under `BANKACCTFROM` vs. under
/// `CCACCTFROM`).
fn extract(xml: &str) -> Result<ParsedOfx, OfxError> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut stack: Vec<String> = Vec::new();
    let mut statements: Vec<OfxStatement> = Vec::new();
    let mut has_investment = false;

    let mut in_statement = false;
    let mut current_kind = StatementKind::Bank;
    let mut current_account = AccountBuilder::default();
    let mut current_currency: Option<String> = None;
    let mut current_transactions: Vec<OfxTransaction> = Vec::new();
    let mut current_ledger_balance = BalanceBuilder::default();
    let mut current_statement_start: Option<String> = None;
    let mut current_statement_end: Option<String> = None;
    let mut current_txn: Option<OfxTransaction> = None;

    loop {
        match reader
            .read_event()
            .map_err(|e| OfxError::Malformed(e.to_string()))?
        {
            Event::Eof => break,
            // A self-closing tag (`Event::Empty`, `<TAG/>`) never gets a
            // matching `Event::End` from quick-xml, so it must never be
            // pushed onto `stack` (nothing would ever pop it) and can never
            // meaningfully be one of the stateful containers below (a real
            // OFX STMTRS/STMTTRN always has children). Real OFX text data
            // arrives as `Event::Text` under a `Event::Start`, so an empty
            // tag carries no leaf value either - safe to simply ignore.
            Event::Empty(_) => {}
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_uppercase();
                match name.as_str() {
                    "INVSTMTMSGSRSV1" | "INVSTMTTRNRS" | "INVSTMTRS" => has_investment = true,
                    "STMTRS" => {
                        in_statement = true;
                        current_kind = StatementKind::Bank;
                        current_account = AccountBuilder::default();
                        current_currency = None;
                        current_transactions = Vec::new();
                        current_ledger_balance = BalanceBuilder::default();
                        current_statement_start = None;
                        current_statement_end = None;
                    }
                    "CCSTMTRS" => {
                        in_statement = true;
                        current_kind = StatementKind::CreditCard;
                        current_account = AccountBuilder::default();
                        current_currency = None;
                        current_transactions = Vec::new();
                        current_ledger_balance = BalanceBuilder::default();
                        current_statement_start = None;
                        current_statement_end = None;
                    }
                    "STMTTRN" if in_statement => {
                        current_txn = Some(OfxTransaction::default());
                    }
                    _ => {}
                }
                stack.push(name);
            }
            Event::End(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_uppercase();
                match name.as_str() {
                    "STMTTRN" => {
                        if let Some(txn) = current_txn.take() {
                            current_transactions.push(txn);
                        }
                    }
                    "STMTRS" | "CCSTMTRS" if in_statement => {
                        let source_account_id =
                            current_account.source_account_id.clone().ok_or_else(|| {
                                OfxError::Malformed(
                                    "a statement is missing its account identifier".to_string(),
                                )
                            })?;
                        statements.push(OfxStatement {
                            account: SourceAccount {
                                bank_id: current_account.bank_id.clone(),
                                source_account_id,
                                source_account_type: current_account.source_account_type.clone(),
                                kind: current_kind,
                                currency: current_currency.clone(),
                            },
                            transactions: std::mem::take(&mut current_transactions),
                            ledger_balance: match (
                                current_ledger_balance.amount.clone(),
                                current_ledger_balance.as_of_date.clone(),
                            ) {
                                (Some(amount), Some(as_of_date)) => {
                                    Some(StatementBalance { amount, as_of_date })
                                }
                                _ => None,
                            },
                            statement_start_date: current_statement_start.clone(),
                            statement_end_date: current_statement_end.clone(),
                        });
                        in_statement = false;
                    }
                    _ => {}
                }
                stack.pop();
            }
            Event::Text(t) => {
                let decoded = t.decode().map_err(|e| OfxError::Malformed(e.to_string()))?;
                let text = quick_xml::escape::unescape(&decoded)
                    .map_err(|e| OfxError::Malformed(e.to_string()))?
                    .trim()
                    .to_string();
                if text.is_empty() {
                    continue;
                }
                let tag = stack.last().cloned().unwrap_or_default();

                if let Some(txn) = current_txn.as_mut() {
                    match tag.as_str() {
                        "TRNTYPE" => txn.trntype = Some(text),
                        "DTPOSTED" => txn.dtposted = Some(text),
                        "DTUSER" => txn.dtuser = Some(text),
                        "TRNAMT" => txn.trnamt = Some(text),
                        "FITID" => txn.fitid = Some(text),
                        "NAME" => txn.name = Some(text),
                        "MEMO" => txn.memo = Some(text),
                        "CHECKNUM" => txn.checknum = Some(text),
                        _ => {}
                    }
                    continue;
                }

                match tag.as_str() {
                    "BANKID" if parent_is(&stack, "BANKACCTFROM") => {
                        current_account.bank_id = Some(text)
                    }
                    "ACCTID"
                        if parent_is(&stack, "BANKACCTFROM") || parent_is(&stack, "CCACCTFROM") =>
                    {
                        current_account.source_account_id = Some(text)
                    }
                    "ACCTTYPE" if parent_is(&stack, "BANKACCTFROM") => {
                        current_account.source_account_type = Some(text)
                    }
                    "CURDEF" if in_statement => current_currency = Some(text),
                    "BALAMT" if parent_is(&stack, "LEDGERBAL") => {
                        current_ledger_balance.amount = Some(text)
                    }
                    "DTASOF" if parent_is(&stack, "LEDGERBAL") => {
                        current_ledger_balance.as_of_date = Some(text)
                    }
                    "DTSTART" if parent_is(&stack, "BANKTRANLIST") => {
                        current_statement_start = Some(text)
                    }
                    "DTEND" if parent_is(&stack, "BANKTRANLIST") => {
                        current_statement_end = Some(text)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    Ok(ParsedOfx {
        statements,
        has_investment_statement: has_investment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFX_1X_CHECKING: &str = r#"OFXHEADER:100
DATA:OFXSGML
VERSION:102
SECURITY:NONE
ENCODING:USASCII
CHARSET:1252
COMPRESSION:NONE
OLDFILEUID:NONE
NEWFILEUID:NONE

<OFX>
<SIGNONMSGSRSV1>
<SONRS>
<STATUS>
<CODE>0
<SEVERITY>INFO
</STATUS>
<DTSERVER>20260630120000
<LANGUAGE>ENG
</SONRS>
</SIGNONMSGSRSV1>
<BANKMSGSRSV1>
<STMTTRNRS>
<TRNUID>1
<STATUS>
<CODE>0
<SEVERITY>INFO
</STATUS>
<STMTRS>
<CURDEF>USD
<BANKACCTFROM>
<BANKID>111000025
<ACCTID>0001112223
<ACCTTYPE>CHECKING
</BANKACCTFROM>
<BANKTRANLIST>
<DTSTART>20260601
<DTEND>20260630
<STMTTRN>
<TRNTYPE>DEBIT
<DTPOSTED>20260601120000
<TRNAMT>-4.50
<FITID>SAMPLE-FITID-001
<NAME>SAMPLE COFFEE MERCHANT
<MEMO>Purchase
</STMTTRN>
<STMTTRN>
<TRNTYPE>CREDIT
<DTPOSTED>20260602120000
<TRNAMT>2000.00
<FITID>SAMPLE-FITID-002
<NAME>SAMPLE EMPLOYER PAYROLL
</STMTTRN>
</BANKTRANLIST>
<LEDGERBAL>
<BALAMT>1995.50
<DTASOF>20260630120000
</LEDGERBAL>
</STMTRS>
</STMTTRNRS>
</BANKMSGSRSV1>
</OFX>
"#;

    const OFX_2X_CHECKING: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<?OFX OFXHEADER="200" VERSION="200" SECURITY="NONE" OLDFILEUID="NONE" NEWFILEUID="NONE"?>
<OFX>
  <SIGNONMSGSRSV1>
    <SONRS>
      <STATUS><CODE>0</CODE><SEVERITY>INFO</SEVERITY></STATUS>
      <DTSERVER>20260630120000</DTSERVER>
      <LANGUAGE>ENG</LANGUAGE>
    </SONRS>
  </SIGNONMSGSRSV1>
  <BANKMSGSRSV1>
    <STMTTRNRS>
      <TRNUID>1</TRNUID>
      <STATUS><CODE>0</CODE><SEVERITY>INFO</SEVERITY></STATUS>
      <STMTRS>
        <CURDEF>USD</CURDEF>
        <BANKACCTFROM>
          <BANKID>111000025</BANKID>
          <ACCTID>0001112223</ACCTID>
          <ACCTTYPE>CHECKING</ACCTTYPE>
        </BANKACCTFROM>
        <BANKTRANLIST>
          <DTSTART>20260601</DTSTART>
          <DTEND>20260630</DTEND>
          <STMTTRN>
            <TRNTYPE>DEBIT</TRNTYPE>
            <DTPOSTED>20260601120000</DTPOSTED>
            <TRNAMT>-4.50</TRNAMT>
            <FITID>SAMPLE-FITID-001</FITID>
            <NAME>SAMPLE COFFEE MERCHANT</NAME>
            <MEMO>Purchase</MEMO>
          </STMTTRN>
        </BANKTRANLIST>
        <LEDGERBAL>
          <BALAMT>-4.50</BALAMT>
          <DTASOF>20260630120000</DTASOF>
        </LEDGERBAL>
      </STMTRS>
    </STMTTRNRS>
  </BANKMSGSRSV1>
</OFX>
"#;

    const QFX_CREDIT_CARD: &str = r#"OFXHEADER:100
DATA:OFXSGML
VERSION:102
SECURITY:NONE
ENCODING:USASCII
CHARSET:1252
COMPRESSION:NONE
OLDFILEUID:NONE
NEWFILEUID:NONE

<OFX>
<SIGNONMSGSRSV1>
<SONRS>
<STATUS>
<CODE>0
<SEVERITY>INFO
</STATUS>
<DTSERVER>20260630120000
<LANGUAGE>ENG
<INTU.BID>00000
</SONRS>
</SIGNONMSGSRSV1>
<CREDITCARDMSGSRSV1>
<CCSTMTTRNRS>
<TRNUID>1
<STATUS>
<CODE>0
<SEVERITY>INFO
</STATUS>
<CCSTMTRS>
<CURDEF>USD
<CCACCTFROM>
<ACCTID>4111111111111111
</CCACCTFROM>
<BANKTRANLIST>
<DTSTART>20260601
<DTEND>20260630
<STMTTRN>
<TRNTYPE>DEBIT
<DTPOSTED>20260605120000
<TRNAMT>52.10
<FITID>SAMPLE-CC-FITID-001
<NAME>SAMPLE GROCERY MERCHANT
</STMTTRN>
<STMTTRN>
<TRNTYPE>CREDIT
<DTPOSTED>20260610120000
<TRNAMT>-200.00
<FITID>SAMPLE-CC-FITID-002
<NAME>SAMPLE PAYMENT THANK YOU
</STMTTRN>
</BANKTRANLIST>
<LEDGERBAL>
<BALAMT>852.10
<DTASOF>20260630120000
</LEDGERBAL>
</CCSTMTRS>
</CCSTMTTRNRS>
</CREDITCARDMSGSRSV1>
</OFX>
"#;

    const INVESTMENT_ONLY: &str = r#"OFXHEADER:100
DATA:OFXSGML
VERSION:102
SECURITY:NONE
ENCODING:USASCII
CHARSET:1252
COMPRESSION:NONE
OLDFILEUID:NONE
NEWFILEUID:NONE

<OFX>
<SIGNONMSGSRSV1>
<SONRS>
<STATUS>
<CODE>0
<SEVERITY>INFO
</STATUS>
<DTSERVER>20260630120000
<LANGUAGE>ENG
</SONRS>
</SIGNONMSGSRSV1>
<INVSTMTMSGSRSV1>
<INVSTMTTRNRS>
<TRNUID>1
<STATUS>
<CODE>0
<SEVERITY>INFO
</STATUS>
<INVSTMTRS>
<DTASOF>20260630120000
<CURDEF>USD
</INVSTMTRS>
</INVSTMTTRNRS>
</INVSTMTMSGSRSV1>
</OFX>
"#;

    #[test]
    fn parses_ofx_1x_sgml_checking_statement() {
        let parsed = parse(OFX_1X_CHECKING).unwrap();
        assert_eq!(parsed.statements.len(), 1);
        let stmt = &parsed.statements[0];
        assert_eq!(stmt.account.kind, StatementKind::Bank);
        assert_eq!(stmt.account.bank_id.as_deref(), Some("111000025"));
        assert_eq!(stmt.account.source_account_id, "0001112223");
        assert_eq!(
            stmt.account.source_account_type.as_deref(),
            Some("CHECKING")
        );
        assert_eq!(stmt.account.currency.as_deref(), Some("USD"));
        assert_eq!(stmt.transactions.len(), 2);
        assert_eq!(
            stmt.transactions[0].fitid.as_deref(),
            Some("SAMPLE-FITID-001")
        );
        assert_eq!(stmt.transactions[0].trnamt.as_deref(), Some("-4.50"));
        assert_eq!(
            stmt.transactions[0].name.as_deref(),
            Some("SAMPLE COFFEE MERCHANT")
        );
        assert_eq!(stmt.transactions[0].memo.as_deref(), Some("Purchase"));
        assert_eq!(
            stmt.transactions[1].memo, None,
            "MEMO is genuinely optional"
        );
        let bal = stmt.ledger_balance.as_ref().unwrap();
        assert_eq!(bal.amount, "1995.50");
        assert_eq!(bal.as_of_date, "20260630120000");
        assert_eq!(stmt.statement_start_date.as_deref(), Some("20260601"));
        assert_eq!(stmt.statement_end_date.as_deref(), Some("20260630"));
    }

    #[test]
    fn statement_period_is_none_when_the_file_never_states_one() {
        let without_period = OFX_1X_CHECKING
            .replace("<DTSTART>20260601\n", "")
            .replace("<DTEND>20260630\n", "");
        let parsed = parse(&without_period).unwrap();
        assert_eq!(parsed.statements[0].statement_start_date, None);
        assert_eq!(parsed.statements[0].statement_end_date, None);
    }

    #[test]
    fn parses_ofx_2x_xml_checking_statement_identically() {
        let parsed = parse(OFX_2X_CHECKING).unwrap();
        assert_eq!(parsed.statements.len(), 1);
        let stmt = &parsed.statements[0];
        assert_eq!(stmt.account.source_account_id, "0001112223");
        assert_eq!(stmt.transactions.len(), 1);
        assert_eq!(
            stmt.transactions[0].fitid.as_deref(),
            Some("SAMPLE-FITID-001")
        );
        assert_eq!(stmt.transactions[0].trnamt.as_deref(), Some("-4.50"));
    }

    #[test]
    fn parses_qfx_credit_card_statement_with_no_bank_id_or_accttype() {
        let parsed = parse(QFX_CREDIT_CARD).unwrap();
        assert_eq!(parsed.statements.len(), 1);
        let stmt = &parsed.statements[0];
        assert_eq!(stmt.account.kind, StatementKind::CreditCard);
        assert_eq!(stmt.account.bank_id, None);
        assert_eq!(stmt.account.source_account_id, "4111111111111111");
        assert_eq!(stmt.account.source_account_type, None);
        assert_eq!(stmt.transactions.len(), 2);
        assert_eq!(stmt.transactions[0].trnamt.as_deref(), Some("52.10"));
        assert_eq!(stmt.transactions[1].trnamt.as_deref(), Some("-200.00"));
    }

    #[test]
    fn investment_only_file_is_a_clear_unsupported_error_not_a_partial_import() {
        let err = parse(INVESTMENT_ONLY).unwrap_err();
        assert!(matches!(err, OfxError::InvestmentOnly));
    }

    #[test]
    fn malformed_file_fails_clearly_rather_than_guessing() {
        let err = parse("not an ofx file at all, just plain text").unwrap_err();
        assert!(matches!(err, OfxError::Malformed(_)));
    }

    #[test]
    fn empty_or_unrecognized_content_is_a_clear_error() {
        assert!(parse("").is_err());
        assert!(parse("<OFX></OFX>").is_err());
    }

    #[test]
    fn unknown_optional_tags_are_ignored_without_changing_meaning() {
        // A proprietary Quicken-style tag (INTU.BID) inside SIGNONMSGSRSV1,
        // plus an unrecognized leaf tag inside STMTTRN - neither should
        // affect extraction of the fields MIDAS actually uses.
        let with_unknown = OFX_1X_CHECKING.replace(
            "<MEMO>Purchase\n</STMTTRN>",
            "<MEMO>Purchase\n<SIC>5411\n</STMTTRN>",
        );
        let parsed = parse(&with_unknown).unwrap();
        assert_eq!(
            parsed.statements[0].transactions[0].memo.as_deref(),
            Some("Purchase")
        );
    }

    #[test]
    fn multiple_statements_in_one_file_are_all_detected() {
        let combined = format!(
            "{}\n{}",
            OFX_1X_CHECKING.trim_end().trim_end_matches("</OFX>"),
            QFX_CREDIT_CARD
                .trim_start()
                .lines()
                .skip_while(|l| !l.starts_with("<OFX>"))
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
        );
        let parsed = parse(&combined).unwrap();
        assert_eq!(parsed.statements.len(), 2);
        assert_eq!(parsed.statements[0].account.kind, StatementKind::Bank);
        assert_eq!(parsed.statements[1].account.kind, StatementKind::CreditCard);
    }
}
