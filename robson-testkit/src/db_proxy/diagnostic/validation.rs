use super::JournalLabel;

pub(super) fn sqlstate(body: &[u8]) -> Option<[u8; 5]> {
    if !body.ends_with(&[0, 0]) {
        return None;
    }
    let mut fields = body.split(|byte| *byte == 0);
    let mut result = None;
    for field in &mut fields {
        if field.first() == Some(&b'C') {
            if result.is_some()
                || field.len() != 6
                || !field[1..].iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            {
                return None;
            }
            result = Some(field[1..].try_into().ok()?);
        }
    }
    result
}

pub(in crate::db_proxy) fn journal_label(startup: &[u8]) -> Option<JournalLabel> {
    let mut fields = startup.get(8..)?.split(|byte| *byte == 0);
    let mut label = None;
    while let (Some(key), Some(value)) = (fields.next(), fields.next()) {
        if key != b"application_name" {
            continue;
        }
        if label.is_some() {
            return None;
        }
        let value = std::str::from_utf8(value).ok()?.strip_prefix("rbx-diag-c")?;
        let (case_text, leg) = value.split_once("-j")?;
        if case_text.is_empty()
            || case_text.len() > 2
            || !case_text.bytes().all(|b| b.is_ascii_digit())
            || !matches!(leg, "0" | "1")
        {
            return None;
        }
        let case = case_text.parse::<u8>().ok()?;
        if case >= 24 || case.to_string() != case_text {
            return None;
        }
        label = Some(JournalLabel { case, leg: leg.parse().ok()? });
    }
    label
}

// Borrow only fixed protocol severity values. Message/detail fields never
// influence attribution and no severity text is retained in a record.
pub(super) fn fatal_response(body: &[u8]) -> bool {
    body.split(|byte| *byte == 0).any(|field| {
        matches!(field.first(), Some(&b'S') | Some(&b'V'))
            && field
                .get(1..)
                .is_some_and(|severity| severity == b"FATAL" || severity == b"PANIC")
    })
}
