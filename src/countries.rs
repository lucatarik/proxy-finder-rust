//! English country name → ISO 3166-1 alpha-2 mapping.
//! At least 60 entries covering the most common countries found in proxy lists.

use std::collections::HashMap;

fn build_table() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    // 70+ entries
    m.insert("afghanistan", "AF");
    m.insert("aland islands", "AX");
    m.insert("albania", "AL");
    m.insert("algeria", "DZ");
    m.insert("american samoa", "AS");
    m.insert("andorra", "AD");
    m.insert("angola", "AO");
    m.insert("anguilla", "AI");
    m.insert("antarctica", "AQ");
    m.insert("antigua and barbuda", "AG");
    m.insert("argentina", "AR");
    m.insert("armenia", "AM");
    m.insert("aruba", "AW");
    m.insert("australia", "AU");
    m.insert("austria", "AT");
    m.insert("azerbaijan", "AZ");
    m.insert("bahamas", "BS");
    m.insert("bahrain", "BH");
    m.insert("bangladesh", "BD");
    m.insert("barbados", "BB");
    m.insert("belarus", "BY");
    m.insert("belgium", "BE");
    m.insert("belize", "BZ");
    m.insert("benin", "BJ");
    m.insert("bermuda", "BM");
    m.insert("bhutan", "BT");
    m.insert("bolivia", "BO");
    m.insert("bosnia and herzegovina", "BA");
    m.insert("botswana", "BW");
    m.insert("brazil", "BR");
    m.insert("british indian ocean territory", "IO");
    m.insert("brunei darussalam", "BN");
    m.insert("bulgaria", "BG");
    m.insert("burkina faso", "BF");
    m.insert("burundi", "BI");
    m.insert("cambodia", "KH");
    m.insert("cameroon", "CM");
    m.insert("canada", "CA");
    m.insert("cape verde", "CV");
    m.insert("cayman islands", "KY");
    m.insert("central african republic", "CF");
    m.insert("chad", "TD");
    m.insert("chile", "CL");
    m.insert("china", "CN");
    m.insert("christmas island", "CX");
    m.insert("cocos (keeling) islands", "CC");
    m.insert("colombia", "CO");
    m.insert("comoros", "KM");
    m.insert("congo", "CD");
    m.insert("cook islands", "CK");
    m.insert("costa rica", "CR");
    m.insert("cote d'ivoire", "CI");
    m.insert("croatia", "HR");
    m.insert("cuba", "CU");
    m.insert("curaçao", "CW");
    m.insert("cyprus", "CY");
    m.insert("czech republic", "CZ");
    m.insert("czechia", "CZ");
    m.insert("denmark", "DK");
    m.insert("djibouti", "DJ");
    m.insert("dominica", "DM");
    m.insert("dominican republic", "DO");
    m.insert("ecuador", "EC");
    m.insert("egypt", "EG");
    m.insert("el salvador", "SV");
    m.insert("equatorial guinea", "GQ");
    m.insert("eritrea", "ER");
    m.insert("estonia", "EE");
    m.insert("ethiopia", "ET");
    m.insert("falkland islands (malvinas)", "FK");
    m.insert("faroe islands", "FO");
    m.insert("fiji", "FJ");
    m.insert("finland", "FI");
    m.insert("france", "FR");
    m.insert("french guiana", "GF");
    m.insert("french polynesia", "PF");
    m.insert("gabon", "GA");
    m.insert("gambia", "GM");
    m.insert("georgia", "GE");
    m.insert("germany", "DE");
    m.insert("ghana", "GH");
    m.insert("gibraltar", "GI");
    m.insert("greece", "GR");
    m.insert("greenland", "GL");
    m.insert("grenada", "GD");
    m.insert("guam", "GU");
    m.insert("guatemala", "GT");
    m.insert("guernsey", "GG");
    m.insert("guinea", "GN");
    m.insert("guinea-bissau", "GW");
    m.insert("guyana", "GY");
    m.insert("haiti", "HT");
    m.insert("heard island and mcdonald islands", "HM");
    m.insert("holy see (vatican city state)", "VA");
    m.insert("honduras", "HN");
    m.insert("hong kong", "HK");
    m.insert("hungary", "HU");
    m.insert("iceland", "IS");
    m.insert("india", "IN");
    m.insert("indonesia", "ID");
    m.insert("iran", "IR");
    m.insert("iraq", "IQ");
    m.insert("ireland", "IE");
    m.insert("isle of man", "IM");
    m.insert("israel", "IL");
    m.insert("italy", "IT");
    m.insert("jamaica", "JM");
    m.insert("japan", "JP");
    m.insert("jersey", "JE");
    m.insert("jordan", "JO");
    m.insert("kazakhstan", "KZ");
    m.insert("kenya", "KE");
    m.insert("kiribati", "KI");
    m.insert("korea, republic of", "KR");
    m.insert("kosovo", "XK");
    m.insert("kuwait", "KW");
    m.insert("kyrgyzstan", "KG");
    m.insert("lao people's democratic republic", "LA");
    m.insert("latvia", "LV");
    m.insert("lebanon", "LB");
    m.insert("lesotho", "LS");
    m.insert("liberia", "LR");
    m.insert("libya", "LY");
    m.insert("liechtenstein", "LI");
    m.insert("lithuania", "LT");
    m.insert("luxembourg", "LU");
    m.insert("macao", "MO");
    m.insert("madagascar", "MG");
    m.insert("malawi", "MW");
    m.insert("malaysia", "MY");
    m.insert("maldives", "MV");
    m.insert("mali", "ML");
    m.insert("malta", "MT");
    m.insert("marshall islands", "MH");
    m.insert("mauritania", "MR");
    m.insert("mauritius", "MU");
    m.insert("mexico", "MX");
    m.insert("moldova", "MD");
    m.insert("monaco", "MC");
    m.insert("mongolia", "MN");
    m.insert("montenegro", "ME");
    m.insert("morocco", "MA");
    m.insert("mozambique", "MZ");
    m.insert("myanmar", "MM");
    m.insert("namibia", "NA");
    m.insert("nauru", "NR");
    m.insert("nepal", "NP");
    m.insert("netherlands", "NL");
    m.insert("new caledonia", "NC");
    m.insert("new zealand", "NZ");
    m.insert("nicaragua", "NI");
    m.insert("niger", "NE");
    m.insert("nigeria", "NG");
    m.insert("niue", "NU");
    m.insert("norfolk island", "NF");
    m.insert("north korea", "KP");
    m.insert("north macedonia", "MK");
    m.insert("northern mariana islands", "MP");
    m.insert("norway", "NO");
    m.insert("oman", "OM");
    m.insert("pakistan", "PK");
    m.insert("palau", "PW");
    m.insert("panama", "PA");
    m.insert("papua new guinea", "PG");
    m.insert("paraguay", "PY");
    m.insert("peru", "PE");
    m.insert("philippines", "PH");
    m.insert("poland", "PL");
    m.insert("portugal", "PT");
    m.insert("qatar", "QA");
    m.insert("romania", "RO");
    m.insert("russia", "RU");
    m.insert("rwanda", "RW");
    m.insert("saint kitts and nevis", "KN");
    m.insert("saint lucia", "LC");
    m.insert("samoa", "WS");
    m.insert("san marino", "SM");
    m.insert("sao tome and principe", "ST");
    m.insert("saudi arabia", "SA");
    m.insert("senegal", "SN");
    m.insert("serbia", "RS");
    m.insert("seychelles", "SC");
    m.insert("sierra leone", "SL");
    m.insert("singapore", "SG");
    m.insert("slovakia", "SK");
    m.insert("slovenia", "SI");
    m.insert("solomon islands", "SB");
    m.insert("somalia", "SO");
    m.insert("south africa", "ZA");
    m.insert("south korea", "KR");
    m.insert("south sudan", "SS");
    m.insert("spain", "ES");
    m.insert("sri lanka", "LK");
    m.insert("sudan", "SD");
    m.insert("suriname", "SR");
    m.insert("sweden", "SE");
    m.insert("switzerland", "CH");
    m.insert("syria", "SY");
    m.insert("taiwan", "TW");
    m.insert("tajikistan", "TJ");
    m.insert("tanzania", "TZ");
    m.insert("thailand", "TH");
    m.insert("timor-leste", "TL");
    m.insert("togo", "TG");
    m.insert("tokelau", "TK");
    m.insert("tonga", "TO");
    m.insert("trinidad and tobago", "TT");
    m.insert("tunisia", "TN");
    m.insert("turkey", "TR");
    m.insert("turkmenistan", "TM");
    m.insert("tuvalu", "TV");
    m.insert("u.s.", "US");
    m.insert("uae", "AE");
    m.insert("uganda", "UG");
    m.insert("uk", "GB");
    m.insert("ukraine", "UA");
    m.insert("united arab emirates", "AE");
    m.insert("united kingdom", "GB");
    m.insert("united states", "US");
    m.insert("uruguay", "UY");
    m.insert("uzbekistan", "UZ");
    m.insert("vanuatu", "VU");
    m.insert("venezuela", "VE");
    m.insert("vietnam", "VN");
    m.insert("yemen", "YE");
    m.insert("zambia", "ZM");
    m.insert("zimbabwe", "ZW");
    m
}

static TABLE: std::sync::LazyLock<HashMap<&'static str, &'static str>> =
    std::sync::LazyLock::new(build_table);

/// Convert an English country name to alpha-2 code.
pub fn name_to_alpha2(name: &str) -> String {
    let t = name.trim().to_lowercase();
    if let Some(code) = TABLE.get(t.as_str()) {
        return (*code).to_uppercase();
    }
    // already alpha-2?
    if t.len() == 2 && t.is_ascii() {
        return t.to_uppercase();
    }
    String::new()
}

mod codes {
    include!("country_codes.rs");
}
pub use codes::{FLAG_COLS, FLAG_H, FLAG_W};

/// Indice di un alpha-2 in `CODES`, che è anche la cella della bandiera nell'atlante.
pub fn flag_index(a2: &str) -> Option<usize> {
    codes::CODES.binary_search_by(|(c, _)| (*c).cmp(a2)).ok()
}

/// Codice da mostrare: alpha-3 se noto, altrimenti il codice com'è, `---` se vuoto.
pub fn alpha3(a2: &str) -> &str {
    match flag_index(a2) {
        Some(i) => codes::CODES[i].1,
        None if a2.is_empty() => "---",
        None => a2,
    }
}

/// Alpha-2 da un codice di 2 o 3 lettere (per i filtri scritti dall'utente).
pub fn to_alpha2(code: &str) -> Option<String> {
    let c = code.trim().to_ascii_uppercase();
    match c.len() {
        2 => Some(c),
        3 => codes::CODES.iter().find(|(_, a3)| *a3 == c).map(|(a2, _)| (*a2).to_string()),
        _ => None,
    }
}

/// Bandiera come emoji (coppia di indicatori regionali), vuota se il codice non è noto.
pub fn flag_emoji(a2: &str) -> String {
    if flag_index(a2).is_none() {
        return String::new();
    }
    a2.chars().filter_map(|c| char::from_u32(0x1F1E6 + (c as u32 - 'A' as u32))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha3_and_flags() {
        assert_eq!(alpha3("IT"), "ITA");
        assert_eq!(alpha3("GB"), "GBR");
        assert_eq!(alpha3("XK"), "XK");
        assert_eq!(alpha3(""), "---");
        assert_eq!(to_alpha2("ita").as_deref(), Some("IT"));
        assert_eq!(to_alpha2("us").as_deref(), Some("US"));
        assert_eq!(to_alpha2("XYZ"), None);
        assert_eq!(flag_emoji("IT"), "\u{1F1EE}\u{1F1F9}");
        assert_eq!(flag_emoji("XK"), "");
    }

    #[test]
    fn test_name_to_alpha2_basic() {
        assert_eq!(name_to_alpha2("Germany"), "DE");
        assert_eq!(name_to_alpha2("france"), "FR");
        assert_eq!(name_to_alpha2("UNITED STATES"), "US");
        assert_eq!(name_to_alpha2("uk"), "GB");
        assert_eq!(name_to_alpha2(""), "");
        assert_eq!(name_to_alpha2("Atlantis"), "");
    }

    #[test]
    fn test_name_to_alpha2_already_alpha2() {
        assert_eq!(name_to_alpha2("DE"), "DE");
        assert_eq!(name_to_alpha2("us"), "US");
    }

    #[test]
    fn test_table_has_at_least_60_entries() {
        assert!(TABLE.len() >= 60, "table has {} entries", TABLE.len());
    }
}
