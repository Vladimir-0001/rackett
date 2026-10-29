// Category names/IDs from pinned Jackett TorznabCatType.cs (GPL-2.0-only).
pub fn category(name: &str) -> Option<u32> {
    match name {
        "Console" => Some(1000),
        "Console/NDS" => Some(1010),
        "Console/PSP" => Some(1020),
        "Console/Wii" => Some(1030),
        "Console/XBox" => Some(1040),
        "Console/XBox 360" => Some(1050),
        "Console/Wiiware" => Some(1060),
        "Console/XBox 360 DLC" => Some(1070),
        "Console/PS3" => Some(1080),
        "Console/Other" => Some(1090),
        "Console/3DS" => Some(1110),
        "Console/PS Vita" => Some(1120),
        "Console/WiiU" => Some(1130),
        "Console/XBox One" => Some(1140),
        "Console/PS4" => Some(1180),
        "Movies" => Some(2000),
        "Movies/Foreign" => Some(2010),
        "Movies/Other" => Some(2020),
        "Movies/SD" => Some(2030),
        "Movies/HD" => Some(2040),
        "Movies/UHD" => Some(2045),
        "Movies/BluRay" => Some(2050),
        "Movies/3D" => Some(2060),
        "Movies/DVD" => Some(2070),
        "Movies/WEB-DL" => Some(2080),
        "Audio" => Some(3000),
        "Audio/MP3" => Some(3010),
        "Audio/Video" => Some(3020),
        "Audio/Audiobook" => Some(3030),
        "Audio/Lossless" => Some(3040),
        "Audio/Other" => Some(3050),
        "Audio/Foreign" => Some(3060),
        "PC" => Some(4000),
        "PC/0day" => Some(4010),
        "PC/ISO" => Some(4020),
        "PC/Mac" => Some(4030),
        "PC/Mobile-Other" => Some(4040),
        "PC/Games" => Some(4050),
        "PC/Mobile-iOS" => Some(4060),
        "PC/Mobile-Android" => Some(4070),
        "TV" => Some(5000),
        "TV/WEB-DL" => Some(5010),
        "TV/Foreign" => Some(5020),
        "TV/SD" => Some(5030),
        "TV/HD" => Some(5040),
        "TV/UHD" => Some(5045),
        "TV/Other" => Some(5050),
        "TV/Sport" => Some(5060),
        "TV/Anime" => Some(5070),
        "TV/Documentary" => Some(5080),
        "XXX" => Some(6000),
        "XXX/DVD" => Some(6010),
        "XXX/WMV" => Some(6020),
        "XXX/XviD" => Some(6030),
        "XXX/x264" => Some(6040),
        "XXX/UHD" => Some(6045),
        "XXX/Pack" => Some(6050),
        "XXX/ImageSet" => Some(6060),
        "XXX/Other" => Some(6070),
        "XXX/SD" => Some(6080),
        "XXX/WEB-DL" => Some(6090),
        "Books" => Some(7000),
        "Books/Mags" => Some(7010),
        "Books/EBook" => Some(7020),
        "Books/Comics" => Some(7030),
        "Books/Technical" => Some(7040),
        "Books/Other" => Some(7050),
        "Books/Foreign" => Some(7060),
        "Other" => Some(8000),
        "Other/Misc" => Some(8010),
        "Other/Hashed" => Some(8020),
        _ => None,
    }
}

// Pinned TorznabCapabilitiesCategories.AddCategoryMapping: int.TryParse first,
// otherwise SHA1(UTF8(id)), BitConverter.ToUInt16 on Jackett's little-endian hosts.
pub(super) fn custom_id(id: &str) -> Result<u32, String> {
    use sha1::{Digest, Sha1};
    // NumberStyles.Integer permits only ASCII whitespace, a sign and decimal digits.
    let trimmed = id.trim_matches([' ', '\t', '\r', '\n', '\x0b', '\x0c']);
    let numeric = trimmed.parse::<i32>().ok();
    let tracker = numeric.unwrap_or_else(|| {
        let hash = Sha1::digest(id.as_bytes());
        i32::from(u16::from_le_bytes([hash[0], hash[1]]))
    });
    tracker
        .checked_add(100_000)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| {
            "Custom category is outside the nonnegative signed 32-bit result range".into()
        })
}

// Only real Torznab children belong to a parent. Custom IDs can also collide
// with standard IDs (negative numeric tracker IDs); unknown IDs are tree roots.
pub(super) fn parent(id: u32) -> Option<u32> {
    match id {
        1010 | 1020 | 1030 | 1040 | 1050 | 1060 | 1070 | 1080 | 1090 | 1110 | 1120 | 1130
        | 1140 | 1180 | 2010 | 2020 | 2030 | 2040 | 2045 | 2050 | 2060 | 2070 | 2080 | 3010
        | 3020 | 3030 | 3040 | 3050 | 3060 | 4010 | 4020 | 4030 | 4040 | 4050 | 4060 | 4070
        | 5010 | 5020 | 5030 | 5040 | 5045 | 5050 | 5060 | 5070 | 5080 | 6010 | 6020 | 6030
        | 6040 | 6045 | 6050 | 6060 | 6070 | 6080 | 6090 | 7010 | 7020 | 7030 | 7040 | 7050
        | 7060 | 8010 | 8020 => Some(id / 1000 * 1000),
        _ => None,
    }
}

// Bounded invariant comparison subset: canonical equivalence, case and final
// sigma. Unlike andmatch's IgnoreNonSpace, category IgnoreCase retains accents.
pub(super) fn matches(a: &str, b: &str) -> Result<bool, String> {
    use unicode_normalization::UnicodeNormalization;
    if a.trim().is_empty() || b.trim().is_empty() {
        return Ok(false);
    }
    if a.is_ascii() && b.is_ascii() {
        return Ok(a.eq_ignore_ascii_case(b));
    }
    static UNSUPPORTED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"[\p{Cf}ßẞÆæŒœĲĳſØøŁłĐđĦħÐðﬀ-ﬆ]").expect("fixed regex")
    });
    if UNSUPPORTED.is_match(a) || UNSUPPORTED.is_match(b) {
        return Err(
            "Invariant category collation expansion/ignorable character unsupported".into(),
        );
    }
    let fold = |s: &str| {
        s.nfd()
            .flat_map(char::to_lowercase)
            .map(|c| if c == 'ς' { 'σ' } else { c })
            .collect::<String>()
    };
    Ok(fold(a) == fold(b))
}
