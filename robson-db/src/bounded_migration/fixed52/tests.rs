use super::*;
#[test]
fn original52_ranges_hashes_and_omissions_are_exact() {
    let migration = crate::MIGRATOR.iter().find(|m| m.version == VERSION).unwrap();
    let chunks = chunks(migration).unwrap();
    assert_eq!(chunks.len(), 52);
    let mut omitted = String::new();
    let mut previous = 0;
    for part in manifest::PARTS {
        omitted.push_str(&ORIGINAL[previous..part.start]);
        previous = part.end;
    }
    omitted.push_str(&ORIGINAL[previous..]);
    assert_eq!(omitted, include_str!("omitted.txt"));
    assert!(chunks[40].contains("DECLARE"));
    assert!(chunks[42].contains("DECLARE"));
}
#[test]
fn fixed_preflight_refuses_count_range_utf8_and_byte_drift() {
    assert!(split(ORIGINAL, &manifest::PARTS[..51]).is_err());
    let mut parts = manifest::PARTS;
    parts[0].start += 1;
    assert!(split(ORIGINAL, &parts).is_err());
    parts = manifest::PARTS;
    parts[0].end = usize::MAX;
    assert!(split(ORIGINAL, &parts).is_err());
    parts = manifest::PARTS;
    parts[1].start = parts[0].start;
    assert!(split(ORIGINAL, &parts).is_err());
    let original = crate::MIGRATOR.iter().find(|m| m.version == VERSION).unwrap();
    let mut changed = original.clone();
    changed.checksum.to_mut()[0] ^= 1;
    assert!(chunks(&changed).is_err());
    changed = original.clone();
    changed.description = "changed".into();
    assert!(chunks(&changed).is_err());
    let bad = [Part {
        start: 1,
        end: 2,
        checksum: manifest::PARTS[0].checksum,
    }; 52];
    assert!(split("é", &bad).is_err());
}
