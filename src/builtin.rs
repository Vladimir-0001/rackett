//! Embedded pinned catalog and conservative runtime eligibility.
use crate::{
    cardigann::{self, Node},
    cardigann_exec::Plan,
    models::{Indexer, SearchQuery},
};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};
include!(concat!(env!("OUT_DIR"), "/builtin_sources.rs"));
pub const PROVENANCE: &str =
    "Jackett/Jackett fe8876d21b93dfda84eb12bd24e1c72e56fb8736; vendor/jackett/SHA256SUMS";
pub const READY_IDS: &[&str] = &[
    "0dayfiles-api",
    "0daykiev",
    "0magnet",
    "1337x",
    "13city",
    "13city-api",
    "1ptbar",
    "1ptbar-api",
    "3dvault-api",
    "4thd",
    "52bt",
    "52pt",
    "Bittorrentfiles",
    "abtorrents",
    "acgrip",
    "acrossthetasman",
    "aftershock",
    "agsvpt",
    "aidoruonline",
    "aither-api",
    "alabala-api",
    "alingpt",
    "alingpt-api",
    "amigosshare",
    "anibt",
    "animebybelka",
    "animelayer",
    "animetorrentsro",
    "animetosho-xyz",
    "animeworld-api",
    "anirena",
    "anisource",
    "anthelion-api",
    "arabafenice",
    "arabicsource-api",
    "arabp2p",
    "arabscene",
    "arabtorrents",
    "asiancinema",
    "asiandvdclub",
    "astratorrent",
    "audiences",
    "audionews",
    "aussierules",
    "azusa",
    "backups",
    "bangumi-moe",
    "baozipt",
    "baozipt-api",
    "beload",
    "best-torrents",
    "bestcore",
    "bibliotik",
    "bigcore",
    "bigfangroup",
    "binarybay",
    "bitagent",
    "bitbazis",
    "bitded",
    "bitgamer",
    "bithumen",
    "bitmagnet",
    "bitporn",
    "bitturk",
    "bjshare",
    "blueroms",
    "bluetorrents",
    "bluraytracker",
    "blutopia-api",
    "booktracker",
    "bootytape",
    "borgzelle",
    "boxingtorrents",
    "btarg",
    "btdirectory",
    "btetree",
    "btnext",
    "btschool",
    "bwtorrents",
    "byrbt",
    "byrutor",
    "c411",
    "cangbaoge",
    "capybarabr",
    "carpathians",
    "carpt",
    "cathoderaytube",
    "catorrent",
    "ceskeforum",
    "cgpeers",
    "cinemageddon",
    "cinemamovies",
    "cinematik",
    "clearjav-api",
    "coastalcrew",
    "comicat",
    "concertos",
    "crabpt",
    "crabpt-api",
    "crackingpatching",
    "crazyspirits-api",
    "crnaberza",
    "crnaberza2fa",
    "crt2fa",
    "cspt",
    "cyanbug",
    "cyclingarchiveclub",
    "czteam-api",
    "damagnet",
    "danishbits-api",
    "darkpeers",
    "dasunerwartete",
    "dasunerwartete-api",
    "datascene-api",
    "daxiangjiao",
    "daxiangjiao-api",
    "deildu",
    "depthstudio",
    "depthstudio-api",
    "desitorrents-api",
    "diablotorrent",
    "digitalcore-api",
    "dimeadozen",
    "dirtybytes-api",
    "discfan",
    "discfan-api",
    "dmhy",
    "docspedia",
    "draupnirr",
    "dreadvault-api",
    "dreamingtree",
    "ds-reloaded",
    "dubhe",
    "dxp",
    "ebookbay",
    "ehentai",
    "empornium",
    "empornium2fa",
    "emuwarez",
    "estone",
    "ex-torrenty",
    "exitorrent-org",
    "exkinoray",
    "explosiv-world",
    "extratorrent-st",
    "extremebits",
    "eztv",
    "eztvl",
    "f1carreras-api",
    "fanoin",
    "fappaizuri",
    "fappaizuri2fa",
    "femdomcult",
    "fenyarnyek-tracker",
    "file-tracker",
    "fileiplay",
    "filemood",
    "finelite",
    "flood",
    "freefarm",
    "freefarm-api",
    "freejavtorrent",
    "funzone-api",
    "futuretorrent",
    "fuzer",
    "g3minitr4ck3r-api",
    "gay-torrents",
    "gaytorrentru",
    "generationfree-api",
    "ggpt",
    "gigatorrents",
    "girotorrent",
    "haidan",
    "haitang",
    "happyfappy",
    "happyfappy2fa",
    "hawke-uno",
    "hd-club",
    "hd-unit3d-api",
    "hdarea",
    "hdbao",
    "hdbao-api",
    "hdc",
    "hddolby",
    "hdfans",
    "hdforever-api",
    "hdgalaktik",
    "hdhome",
    "hdkylin",
    "hdolimpo-api",
    "hdonly-api",
    "hdroute",
    "hdsky",
    "hdspace",
    "hdspacecookie",
    "hdtime",
    "hdtorrents",
    "hdtorrentsit",
    "hdturk",
    "hdu",
    "hdvideo-api",
    "hdzero",
    "hebits",
    "hellashut",
    "hellashut-api",
    "hellenic-hd-api",
    "helltorrents",
    "herobit",
    "hhd-api",
    "houseofdevil",
    "hqmusic",
    "hudbt",
    "huntorrent",
    "hxpt",
    "ilcorsaroblu",
    "immortuos",
    "indietorrents",
    "infinityhd-api",
    "infire",
    "insanetracker",
    "internetarchive",
    "itatorrents",
    "jme-reunit3d-api",
    "jpopsuki",
    "jptv4us-api",
    "kamept",
    "keepfriends",
    "kickasstorrents-to",
    "kickasstorrents-ws",
    "kinorun",
    "korsar",
    "krazyzone",
    "kufei",
    "kufei-api",
    "kufirc",
    "kufirc2fa",
    "lajidui",
    "lajidui-api",
    "lastdigitalunderground",
    "lastfiles",
    "lat-team-api",
    "learnflakes",
    "leech24",
    "lemonhd-net",
    "lemonhd-net-api",
    "lesaloon",
    "libranet",
    "limetorrents",
    "linkomanija",
    "linuxtracker",
    "locadora",
    "longpt",
    "longpt-api",
    "losslessclub",
    "lp-bits",
    "lst",
    "luckpt",
    "luminarr-api",
    "mactorrentsdownload",
    "madsrevolution",
    "magico",
    "magnetcat",
    "magnetdownload",
    "magnetz",
    "majomparade",
    "malayabits-api",
    "march",
    "marinetracker",
    "matrix",
    "mazepa",
    "mdan",
    "megapeer",
    "memphis",
    "mesevilag",
    "metalguru",
    "metaltracker",
    "midnightscene",
    "mikan",
    "milkie",
    "milnueve-api",
    "miobt",
    "mixtapetorrent",
    "mma-torrents",
    "mnv",
    "mojblink",
    "momentpt",
    "monikadesign-api",
    "mousebits",
    "musebootlegs",
    "musopia",
    "musopia-api",
    "muxuege",
    "mvgroupforum",
    "mvgroupmain",
    "mypornclub",
    "myspleen",
    "nanyangpt",
    "ncore",
    "nekobt",
    "nethd",
    "new-team",
    "newheaven",
    "newretro",
    "newstudio",
    "newstudiol",
    "nicept",
    "nicept-api",
    "nipponsei",
    "nirvana",
    "njtupt",
    "nobs",
    "noname-club",
    "noname-clubl",
    "nordicbytes",
    "nordicquality",
    "nostradamus",
    "novahd",
    "nyaasi",
    "okpt",
    "oldgreektracker",
    "oldtoonsworld",
    "onejav",
    "onlyencodes-api",
    "opencd",
    "opensharing",
    "order66",
    "oshenpt",
    "oshenpt-api",
    "ourbits",
    "p2pbg",
    "panda",
    "pandacd",
    "parabellumhd-api",
    "party-tracker",
    "pctorrent",
    "peeratiko",
    "peergarden",
    "peersfm",
    "periodical",
    "phoenixproject",
    "phoenixpt",
    "phoenixpt-api",
    "pier720",
    "pignetwork",
    "pixelcove",
    "pixelcove2fa",
    "playletpt",
    "plugintorrent",
    "podzemlje",
    "polishtorrent",
    "pornolab",
    "pornorip",
    "pornotorrent",
    "pornrips",
    "pornxlab",
    "portugas-api",
    "postman",
    "postman-api",
    "proaudiotorrents",
    "proporno",
    "ptcafe",
    "ptcafe-api",
    "ptcc",
    "pterclub",
    "ptfans",
    "ptfiles",
    "ptgtk",
    "ptgtk-api",
    "pthome",
    "ptitzmx",
    "ptlgs",
    "ptsbao",
    "ptskit",
    "ptskit-api",
    "pttey",
    "pttime",
    "ptzone",
    "ptzone-api",
    "punkshorror",
    "puntotorrent",
    "pussytorrents",
    "putao",
    "pwtorrents",
    "pwtorrents2fa",
    "qingwa",
    "r3vwtf",
    "racing4everyone-api",
    "racing4us",
    "racingforme",
    "railgunpt",
    "rastastugan-api",
    "redstartorrent",
    "reelflix-api",
    "retromoviesclub-api",
    "retrotoon",
    "rgfootball",
    "rintor",
    "rintornet",
    "riperam",
    "rockethd",
    "rousi-pro",
    "rudub",
    "rustorka",
    "rutor",
    "rutracker-ru",
    "samaritano",
    "sapibr",
    "sbpt",
    "scenerush",
    "scenetime-api",
    "seedcore-api",
    "seedfile",
    "seedpool-api",
    "sewerpt",
    "sextorrent-api",
    "sexypics",
    "shadowbit",
    "shanaproject",
    "shareisland-api",
    "showrss",
    "siambit",
    "siqi",
    "skidrowrepack",
    "skipthecommercials-api",
    "sktorrent",
    "sktorrent-org",
    "snowpt",
    "sosulki",
    "soulvoice",
    "soulvoice-api",
    "speedmasterhd",
    "spiritofrevolution",
    "sportscora",
    "sportscult",
    "springsunday",
    "stellarwinds",
    "sukebeinyaasi",
    "superbits",
    "swarmazon-api",
    "tangpt",
    "tapochek",
    "tasmanit",
    "teamflix",
    "teamhd",
    "teamos",
    "thebrothers",
    "thecrazyones",
    "theempire",
    "thefallingangels",
    "thegeeks",
    "thekitchen",
    "theleachzone-api",
    "themixingbowl",
    "thenewfun",
    "thenewheaven",
    "theoccult",
    "theoldschool-api",
    "thepiratebay",
    "theplace",
    "therarbg",
    "theshow",
    "thevault",
    "thewordmusic",
    "thorhub-api",
    "tjupt",
    "tlfbits",
    "tmghub",
    "tokyotosho",
    "tormac",
    "tornado",
    "torrent-heaven",
    "torrent-pirat",
    "torrent-turk",
    "torrent9",
    "torrentbd",
    "torrentby",
    "torrentbyte",
    "torrentccf",
    "torrentclaw",
    "torrentcore",
    "torrentdd",
    "torrentdownload",
    "torrentdownloads",
    "torrenteros-api",
    "torrentgalaxyclone",
    "torrenthaven-api",
    "torrenthr-api",
    "torrenting",
    "torrentkitty",
    "torrentleech",
    "torrentlt",
    "torrentmasters",
    "torrentproject2",
    "torrentsome",
    "torrenttip",
    "torrenttrader",
    "totheglory",
    "totheglory2fa",
    "tr4ker",
    "trackermk",
    "trackerzero",
    "traht",
    "trancetraffic",
    "trezzor",
    "trezzorcookie",
    "tribalmixes",
    "turkseed-api",
    "turktorrent",
    "u2",
    "u3c3",
    "uatorrent",
    "ubits",
    "uindex",
    "ultrabits-api",
    "ultrahd",
    "ultrahd-api",
    "unbreakable",
    "unlimitz",
    "uploadcx",
    "upscalevault",
    "utopia",
    "uztracker",
    "vault-network",
    "vc-lib",
    "vc-lib-api",
    "vietmediaf",
    "vsthouse",
    "vstorrent",
    "vsttorrents",
    "whiteangel",
    "wihd",
    "wintersakura",
    "world-of-tomorrow-api",
    "world-torrent",
    "xdy",
    "xdy-api",
    "xingtan",
    "xingwan",
    "xingyung",
    "xloli",
    "xloli-api",
    "xtorrenty",
    "xtremebytes",
    "xwtclassics",
    "xwtclassics2fa",
    "xwtorrents",
    "xwtorrents2fa",
    "xxxclub",
    "xxxtor",
    "yts",
    "yuscene-api",
    "zamundalife",
    "zappateers",
    "zenith",
    "zmpt",
    "zmpt-api",
    "zrpt",
    "ztracker",
];
/// Public definitions whose search path is an absolute URL on a declared API host. Their
/// searches may use that one origin besides the site; see [`api_origin`] and
/// docs/api-origins.md. This admits no runtime: each is still gated by [`READY_IDS`].
pub const API_ORIGIN_IDS: &[&str] = &["animetosho-xyz", "thepiratebay", "yts"];
/// Definitions whose source may carry a saved cookie and use the opt-in challenge solver.
/// This admits no runtime: each is still gated by [`READY_IDS`].
pub const CHALLENGE_IDS: &[&str] = &["1337x", "eztv", "tokyotosho"];
/// Cookie-login presets with complete engine/search/download lifecycle fixtures.
pub const COOKIE_IDS: &[&str] = &[
    "13city",
    "1ptbar",
    "abtorrents",
    "aftershock",
    "agsvpt",
    "aidoruonline",
    "alingpt",
    "amigosshare",
    "animebybelka",
    "asiandvdclub",
    "audiences",
    "azusa",
    "baozipt",
    "beload",
    "bigcore",
    "bitbazis",
    "bithumen",
    "bjshare",
    "boxingtorrents",
    "byrbt",
    "cangbaoge",
    "cgpeers",
    "cinemamovies",
    "crnaberza2fa",
    "crt2fa",
    "cspt",
    "daxiangjiao",
    "depthstudio",
    "docspedia",
    "ds-reloaded",
    "dubhe",
    "empornium2fa",
    "ex-torrenty",
    "exitorrent-org",
    "fappaizuri2fa",
    "freefarm",
    "fuzer",
    "gaytorrentru",
    "ggpt",
    "happyfappy2fa",
    "hdarea",
    "hdbao",
    "hddolby",
    "hdgalaktik",
    "hdhome",
    "hdkylin",
    "hdsky",
    "hdspacecookie",
    "hdtime",
    "hdturk",
    "hebits",
    "herobit",
    "hqmusic",
    "huntorrent",
    "hxpt",
    "kamept",
    "keepfriends",
    "korsar",
    "krazyzone",
    "kufei",
    "kufirc2fa",
    "lajidui",
    "learnflakes",
    "leech24",
    "lp-bits",
    "luckpt",
    "majomparade",
    "march",
    "matrix",
    "mma-torrents",
    "momentpt",
    "musopia",
    "muxuege",
    "myspleen",
    "nanyangpt",
    "newheaven",
    "nicept",
    "noname-clubl",
    "novahd",
    "okpt",
    "opencd",
    "order66",
    "ourbits",
    "panda",
    "phoenixpt",
    "pier720",
    "pignetwork",
    "pixelcove2fa",
    "playletpt",
    "pornorip",
    "proaudiotorrents",
    "ptcc",
    "pterclub",
    "ptfans",
    "pthome",
    "ptsbao",
    "ptskit",
    "pttey",
    "ptzone",
    "punkshorror",
    "pwtorrents2fa",
    "qingwa",
    "rousi-pro",
    "rustorka",
    "siqi",
    "springsunday",
    "superbits",
    "tangpt",
    "teamhd",
    "teamos",
    "theempire",
    "thegeeks",
    "thekitchen",
    "theoccult",
    "theplace",
    "theshow",
    "thevault",
    "tjupt",
    "tormac",
    "torrent-turk",
    "torrentbd",
    "torrentccf",
    "torrenting",
    "torrentmasters",
    "totheglory2fa",
    "trezzorcookie",
    "turktorrent",
    "u2",
    "ubits",
    "ultrahd",
    "vc-lib",
    "xdy",
    "xingwan",
    "xingyung",
    "xloli",
    "xtorrenty",
    "xwtclassics2fa",
    "xwtorrents2fa",
    "zmpt",
    "zrpt",
];

pub(crate) fn cookie_session(
    i: &Indexer,
) -> Result<Option<crate::builtin_runtime::auth::CookieSession>> {
    if !id(i).is_ok_and(|id| {
        COOKIE_IDS.contains(&id) || FORM_IDS.contains(&id) || POST_IDS.contains(&id)
    }) {
        return Ok(None);
    }
    let r = root(id(i)?)?;
    if FORM_IDS.contains(&id(i)?) || POST_IDS.contains(&id(i)?) {
        let settings = i
            .definition
            .as_ref()
            .and_then(|d| d.get("settings"))
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow!("This preset requires form credentials"))?
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        return Ok(Some(crate::builtin_runtime::auth::CookieSession::new_form(
            crate::builtin_runtime::form_auth::FormPlan::compile(&r)?,
            &i.url,
            &settings,
            request_delay_ms(&r, i.request_delay_ms)?,
            120,
        )?));
    }
    let settings = i
        .definition
        .as_ref()
        .and_then(|d| d.get("settings"))
        .and_then(Value::as_object);
    // Text settings that declared search headers render (hdturk's `csrf_token`).
    let plan = crate::builtin_runtime::auth::CookiePlan::compile(&r)?
        .with_user_agent(
            settings
                .and_then(|s| s.get("useragent"))
                .and_then(Value::as_str),
        )?
        .with_settings(settings.unwrap_or(&serde_json::Map::new()))?;
    let cookie = i
        .definition
        .as_ref()
        .and_then(|d| d.get("settings"))
        .and_then(|s| s.get("cookie"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("This preset requires a cookie setting"))?;
    Ok(Some(crate::builtin_runtime::auth::CookieSession::new(
        plan,
        &i.url,
        cookie,
        request_delay_ms(&r, i.request_delay_ms)?,
        120,
    )?))
}
/// Form-login presets with source-pinned engine/search/download/restart fixtures.
pub const FORM_IDS: &[&str] = &[
    "4thd",
    "52pt",
    "animelayer",
    "animetorrentsro",
    "arabp2p",
    "aussierules",
    "bitturk",
    "bluraytracker",
    "booktracker",
    "btnext",
    "btschool",
    "carpt",
    "cathoderaytube",
    "coastalcrew",
    "comicat",
    "crabpt",
    "crnaberza",
    "cyanbug",
    "dasunerwartete",
    "diablotorrent",
    "discfan",
    "dreamingtree",
    "empornium",
    "explosiv-world",
    "extremebits",
    "femdomcult",
    "file-tracker",
    "finelite",
    "gigatorrents",
    "haidan",
    "haitang",
    "happyfappy",
    "hdc",
    "hdfans",
    "hdtorrents",
    "hdu",
    "hellashut",
    "helltorrents",
    "hudbt",
    "ilcorsaroblu",
    "immortuos",
    "indietorrents",
    "infire",
    "kufirc",
    "lastfiles",
    "lemonhd-net",
    "lesaloon",
    "longpt",
    "magico",
    "magnetz",
    "marinetracker",
    "mesevilag",
    "miobt",
    "mojblink",
    "mousebits",
    "musebootlegs",
    "mvgroupforum",
    "mvgroupmain",
    "new-team",
    "newretro",
    "njtupt",
    "oshenpt",
    "p2pbg",
    "peeratiko",
    "pixelcove",
    "pornolab",
    "postman",
    "proporno",
    "ptcafe",
    "ptfiles",
    "ptgtk",
    "ptitzmx",
    "ptlgs",
    "putao",
    "r3vwtf",
    "railgunpt",
    "rgfootball",
    "riperam",
    "sbpt",
    "sewerpt",
    "sktorrent",
    "snowpt",
    "soulvoice",
    "speedmasterhd",
    "spiritofrevolution",
    "tapochek",
    "thebrothers",
    "thecrazyones",
    "thefallingangels",
    "thenewheaven",
    "thewordmusic",
    "tlfbits",
    "tmghub",
    "tornado",
    "torrent-heaven",
    "torrentleech",
    "torrenttrader",
    "totheglory",
    "uatorrent",
    "unbreakable",
    "unlimitz",
    "uztracker",
    "wihd",
    "wintersakura",
    "xingtan",
    "zappateers",
    "ztracker",
];
/// Credential-POST presets with pinned layouts and complete authenticated lifecycle tests.
/// API-key presets with a `login: get` probe and complete fixture lifecycle tests.
pub const GET_IDS: &[&str] = &[
    "alabala-api",
    "anthelion-api",
    "astratorrent",
    "binarybay",
    "bitagent",
    "bluetorrents",
    "c411",
    "crazyspirits-api",
    "czteam-api",
    "desitorrents-api",
    "digitalcore-api",
    "dirtybytes-api",
    "draupnirr",
    "fileiplay",
    "flood",
    "g3minitr4ck3r-api",
    "generationfree-api",
    "hdforever-api",
    "hdolimpo-api",
    "hdonly-api",
    "hellashut-api",
    "memphis",
    "nostradamus",
    "periodical",
    "polishtorrent",
    "postman-api",
    "retrotoon",
    "scenetime-api",
    "shadowbit",
    "sportscora",
    "stellarwinds",
    "swarmazon-api",
    "teamflix",
    "theoldschool-api",
    "torrentclaw",
    "tr4ker",
    "trackerzero",
    "ultrabits-api",
    "world-of-tomorrow-api",
];
/// The `login: get` probe of an admitted [`GET_IDS`] preset, with its configured URL.
pub(crate) fn get_login(
    i: &Indexer,
) -> Result<Option<(crate::builtin_runtime::get_login::GetLogin, url::Url)>> {
    let Ok(id) = id(i) else {
        return Ok(None);
    };
    if !GET_IDS.contains(&id) {
        return Ok(None);
    }
    let login = crate::builtin_runtime::get_login::GetLogin::compile(&root(id)?)?;
    let settings: BTreeMap<String, Value> = i
        .definition
        .as_ref()
        .and_then(|d| d.get("settings"))
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let base = url::Url::parse(&i.url).map_err(|_| anyhow!("Invalid source URL"))?;
    let url = login.url(&base, &settings)?;
    Ok(Some((login, url)))
}
pub const POST_IDS: &[&str] = &[
    "0daykiev",
    "Bittorrentfiles",
    "acrossthetasman",
    "arabafenice",
    "arabscene",
    "arabtorrents",
    "audionews",
    "backups",
    "best-torrents",
    "bestcore",
    "bibliotik",
    "bitded",
    "bitgamer",
    "bootytape",
    "borgzelle",
    "btarg",
    "bwtorrents",
    "carpathians",
    "ceskeforum",
    "cinemageddon",
    "cyclingarchiveclub",
    "deildu",
    "dimeadozen",
    "dxp",
    "estone",
    "exkinoray",
    "eztvl",
    "fanoin",
    "fappaizuri",
    "fenyarnyek-tracker",
    "futuretorrent",
    "gay-torrents",
    "girotorrent",
    "hd-club",
    "hdroute",
    "hdspace",
    "houseofdevil",
    "insanetracker",
    "jpopsuki",
    "kinorun",
    "libranet",
    "linkomanija",
    "losslessclub",
    "madsrevolution",
    "mazepa",
    "mdan",
    "metalguru",
    "metaltracker",
    "mnv",
    "ncore",
    "nethd",
    "newstudiol",
    "oldgreektracker",
    "party-tracker",
    "peersfm",
    "phoenixproject",
    "pttime",
    "puntotorrent",
    "pussytorrents",
    "pwtorrents",
    "racing4us",
    "racingforme",
    "redstartorrent",
    "rintor",
    "rudub",
    "scenerush",
    "seedfile",
    "siambit",
    "sktorrent-org",
    "sportscult",
    "tasmanit",
    "themixingbowl",
    "thenewfun",
    "torrentdd",
    "torrentlt",
    "trackermk",
    "traht",
    "trancetraffic",
    "trezzor",
    "tribalmixes",
    "vault-network",
    "whiteangel",
    "xtremebytes",
    "xwtclassics",
    "xwtorrents",
];
/// Share the same pinned minimum between authenticated and ordinary source requests.
fn request_delay_ms(root: &Node, configured: u64) -> Result<u64> {
    let delay = root
        .get("requestDelay")
        .map(|n| {
            n.text()
                .ok_or_else(|| anyhow!("Invalid pinned delay"))?
                .parse::<f64>()
                .map_err(|_| anyhow!("Invalid pinned delay"))
        })
        .transpose()?
        .unwrap_or(0.);
    if !delay.is_finite() || !(0. ..=60.).contains(&delay) || configured > 60000 {
        bail!("Pinned request delay exceeds runtime bounds");
    }
    Ok(((delay * 1000.).ceil() as u64).max(configured))
}
/// Source-pinned UNIT3D API presets with the bounded GET authentication schema.
pub const API_IDS: &[&str] = &[
    "0dayfiles-api",
    "3dvault-api",
    "aither-api",
    "animeworld-api",
    "arabicsource-api",
    "asiancinema",
    "bitporn",
    "blutopia-api",
    "capybarabr",
    "cinematik",
    "clearjav-api",
    "concertos",
    "danishbits-api",
    "darkpeers",
    "datascene-api",
    "dreadvault-api",
    "emuwarez",
    "f1carreras-api",
    "funzone-api",
    "hawke-uno",
    "hd-unit3d-api",
    "hdtorrentsit",
    "hdzero",
    "hellenic-hd-api",
    "hhd-api",
    "infinityhd-api",
    "itatorrents",
    "jme-reunit3d-api",
    "jptv4us-api",
    "lastdigitalunderground",
    "lat-team-api",
    "locadora",
    "lst",
    "luminarr-api",
    "malayabits-api",
    "midnightscene",
    "milnueve-api",
    "monikadesign-api",
    "nirvana",
    "nobs",
    "nordicbytes",
    "nordicquality",
    "oldtoonsworld",
    "onlyencodes-api",
    "parabellumhd-api",
    "peergarden",
    "podzemlje",
    "portugas-api",
    "racing4everyone-api",
    "rastastugan-api",
    "reelflix-api",
    "retromoviesclub-api",
    "rockethd",
    "samaritano",
    "sapibr",
    "seedcore-api",
    "seedpool-api",
    "sextorrent-api",
    "shareisland-api",
    "skipthecommercials-api",
    "theleachzone-api",
    "thorhub-api",
    "torrenteros-api",
    "torrenthaven-api",
    "torrenthr-api",
    "turkseed-api",
    "uploadcx",
    "upscalevault",
    "utopia",
    "vietmediaf",
    "yuscene-api",
    "zenith",
];
/// Source-pinned NexusPHP v1.9 API presets with independent runtime fixtures.
pub const NEXUS_API_IDS: &[&str] = &[
    "13city-api",
    "1ptbar-api",
    "alingpt-api",
    "baozipt-api",
    "crabpt-api",
    "daxiangjiao-api",
    "depthstudio-api",
    "discfan-api",
    "freefarm-api",
    "hdbao-api",
    "hdvideo-api",
    "kufei-api",
    "lajidui-api",
    "lemonhd-net-api",
    "longpt-api",
    "musopia-api",
    "nicept-api",
    "oshenpt-api",
    "phoenixpt-api",
    "ptcafe-api",
    "ptgtk-api",
    "ptskit-api",
    "ptzone-api",
    "soulvoice-api",
    "ultrahd-api",
    "vc-lib-api",
    "xdy-api",
    "xloli-api",
    "zmpt-api",
];
pub(crate) fn api_session(
    i: &Indexer,
) -> Result<Option<crate::builtin_runtime::api_auth::ApiSession>> {
    let Ok(id) = id(i) else {
        return Ok(None);
    };
    if !API_IDS.contains(&id) && !NEXUS_API_IDS.contains(&id) {
        return Ok(None);
    }
    let key = i
        .definition
        .as_ref()
        .and_then(|d| d.get("settings"))
        .and_then(|s| s.get("apikey"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if NEXUS_API_IDS.contains(&id) {
        let passkey = i
            .definition
            .as_ref()
            .and_then(|d| d.get("settings"))
            .and_then(|s| s.get("passkey"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if passkey.is_empty()
            || passkey.len() > 4096
            || passkey.bytes().any(|b| !b.is_ascii_graphic())
        {
            bail!("NexusPHP API preset requires a nonempty passkey without whitespace");
        }
    }
    Ok(Some(crate::builtin_runtime::api_auth::ApiSession::new(
        &root(id)?,
        &i.url,
        key,
    )?))
}

pub const CHECKSUMS: &str = include_str!("../vendor/jackett/SHA256SUMS");
fn root(id: &str) -> Result<Node> {
    let n = SOURCES
        .binary_search_by_key(&id, |(id, _)| *id)
        .map_err(|_| anyhow!("Unknown bundled definition"))?;
    cardigann::parse(SOURCES[n].1, cardigann::Limits::default())
        .map_err(|_| anyhow!("Invalid bundled definition"))
}
/// The bundled definition's validated `certificates` pins (see [`crate::pinned_tls`]);
/// `None` for unpinned or non-bundled sources.
pub(crate) fn certificates(i: &Indexer) -> Result<Option<crate::pinned_tls::Pins>> {
    match id(i).and_then(root) {
        Ok(r) => crate::pinned_tls::Pins::compile(&r),
        Err(_) => Ok(None),
    }
}
/// Which link the fallback download selector of [`DetailMagnet`] is configured to select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    Magnet,
    ITorrents,
}
/// The only admitted Cardigann `download` block, 1337x's: on the release's detail page,
/// select the first `ul li a[href*="<primarydownloadlink>"]`, else the fallback setting's
/// link, take `href`, and rewrite [`ITORRENTS_REWRITE`]. Only a magnet primary is
/// resolved; iTorrents `.torrent` links are cross-origin and never fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetailMagnet {
    pub fallback: Fallback,
}
/// The fixed `replace` filter of both admitted download selectors.
pub const ITORRENTS_REWRITE: [&str; 2] = ["http://itorrents.org/", "https://itorrents.net/"];
/// Select values of both download link settings, in sorted order.
const LINK_OPTIONS: [&str; 2] = ["//itorrents.", "magnet:"];
const LINK_SETTINGS: [&str; 2] = ["primarydownloadlink", "fallbackdownloadlink"];
fn sorted_keys(n: &Node) -> Option<Vec<&str>> {
    let mut keys = n
        .pairs()?
        .iter()
        .map(|(k, _)| k.text())
        .collect::<Option<Vec<_>>>()?;
    keys.sort_unstable();
    Some(keys)
}
/// The one declaration of a download link setting: a select over exactly
/// [`LINK_OPTIONS`] whose default is one of them.
fn link_setting<'a>(r: &'a Node, name: &str) -> Option<&'a Node> {
    let mut found = r
        .get("settings")?
        .items()?
        .iter()
        .filter(|s| s.string("name") == Some(name));
    let s = found.next().filter(|_| found.next().is_none())?;
    let options = sorted_keys(s.get("options")?)?;
    (s.string("type") == Some("select")
        && options == LINK_OPTIONS
        && s.string("default")
            .is_some_and(|d| LINK_OPTIONS.contains(&d)))
    .then_some(s)
}
/// The general `download.selectors` resolver of a ready definition, compiled from the
/// pinned source on demand. `None` when it has no download block or uses [`DetailMagnet`].
pub fn download_selectors(
    i: &Indexer,
) -> Result<Option<crate::builtin_runtime::download::DownloadSelectors>> {
    let r = root(id(i)?)?;
    if r.get("download").is_none() || detail_magnet_shape(&r) {
        return Ok(None);
    }
    crate::builtin_runtime::download::DownloadSelectors::compile(&r).map(Some)
}
/// Whether `r`'s `download` block is exactly the admitted [`DetailMagnet`] shape. Any
/// other selector, filter, key, header or setting declaration keeps it blocked.
fn detail_magnet_shape(r: &Node) -> bool {
    let Some(download) = r.get("download") else {
        return false;
    };
    let selectors = download.get("selectors").and_then(Node::items);
    sorted_keys(download).as_deref() == Some(&["selectors"][..])
        // Upstream sends search headers with download requests; none are declared here.
        && r.get("search").is_some_and(|s| s.get("headers").is_none())
        && LINK_SETTINGS.iter().all(|name| link_setting(r, name).is_some())
        && selectors.is_some_and(|selectors| {
            selectors.len() == LINK_SETTINGS.len()
                && selectors.iter().zip(LINK_SETTINGS).all(|(s, name)| {
                    let filters = s.get("filters").and_then(Node::items).unwrap_or_default();
                    sorted_keys(s).as_deref() == Some(&["attribute", "filters", "selector"][..])
                        && s.string("selector")
                            == Some(&format!("ul li a[href*=\"{{{{ .Config.{name} }}}}\"]"))
                        && s.string("attribute") == Some("href")
                        && filters.len() == 1
                        && sorted_keys(&filters[0]).as_deref() == Some(&["args", "name"][..])
                        && filters[0].string("name") == Some("replace")
                        && filters[0].get("args").and_then(Node::items).is_some_and(|args| {
                            args.iter().map(Node::text).eq(ITORRENTS_REWRITE.map(Some))
                        })
                })
        })
}
// The embedded catalog is keyed by pinned filenames. Four UNIT3D files retain
// an older filename while their YAML id carries the -api suffix; AniRena has
// a mixed-case YAML id. Keep the filename as the single catalog identity. Do not create
// duplicate catalog entries or confuse their runtime gate with the display id.
fn catalog_id(r: &Node) -> &str {
    match r.string("id").unwrap_or("") {
        "aniRena" => "anirena",
        "darkpeers-api" => "darkpeers",
        "hdzero-api" => "hdzero",
        "nordicquality-api" => "nordicquality",
        "upscalevault-api" => "upscalevault",
        id => id,
    }
}
fn eligible(r: &Node) -> Result<Plan> {
    let p = Plan::compile(r)?;
    let api = (API_IDS.contains(&catalog_id(r)) || NEXUS_API_IDS.contains(&catalog_id(r)))
        && crate::builtin_runtime::api_auth::validate_definition(r).is_ok();
    let cookie = COOKIE_IDS.contains(&r.string("id").unwrap_or(""))
        && crate::builtin_runtime::auth::CookiePlan::compile(r).is_ok();
    let form = (FORM_IDS.contains(&catalog_id(r)) || POST_IDS.contains(&catalog_id(r)))
        && crate::builtin_runtime::form_auth::FormPlan::compile(r).is_ok();
    let get = GET_IDS.contains(&catalog_id(r))
        && crate::builtin_runtime::get_login::GetLogin::compile(r).is_ok();
    let pinned = crate::pinned_tls::Pins::compile(r).is_ok_and(|p| p.is_some());
    if let Some(b) = p.blockers().iter().find(|b| {
        !(b.path == "$.download"
            && (detail_magnet_shape(r)
                || crate::builtin_runtime::download::DownloadSelectors::compile(r).is_ok())
            || b.path == "$.login" && (cookie || api || form || get)
            // Engine and session clients install well-formed pins (see pinned_tls.rs).
            || b.path == "$.certificates" && pinned)
    }) {
        bail!("{}: {}", b.path, b.feature);
    }
    // Plan::compile admitted the definition charset; every runtime decoder uses it.
    for s in r.get("settings").and_then(Node::items).unwrap_or_default() {
        if ![
            "text",
            "password",
            "select",
            "checkbox",
            "multi-select",
            "info",
            "info_flaresolverr",
            "info_cookie",
            "info_useragent",
            "info_category_8000",
        ]
        .contains(&s.string("type").unwrap_or("text"))
        {
            bail!("Unsupported setting type");
        }
    }
    Ok(p)
}
fn ready(r: &Node) -> Result<Plan> {
    let p = eligible(r)?;
    if !READY_IDS.contains(&catalog_id(r)) {
        bail!("Runtime integration has not been fixture-validated for this definition");
    }
    Ok(p)
}
/// The one API origin an [`API_ORIGIN_IDS`] search may use besides the site, as a
/// `scheme://host[:port]/` base. Upstream resolves an absolute search path as is
/// (`resolvePath`, `CardigannIndexer.cs:1436`, `:1543`). Every declared search path must
/// start with `http://` or `https://`, and its host must be a literal or exactly one
/// `{{ .Config.name }}` setting, whose configured (or default) value must render to that
/// same non-empty host. Requests are rendered with an empty query, so a query value never
/// chooses the origin. `None` when the definition is not listed or the configured host
/// is the site's own origin.
fn api_origin(
    r: &Node,
    plan: &Plan,
    settings: &BTreeMap<String, Value>,
    site: &url::Url,
) -> Result<Option<url::Url>> {
    if !API_ORIGIN_IDS.contains(&catalog_id(r)) {
        return Ok(None);
    }
    let invalid = || anyhow!("Invalid API host setting");
    let search = r.get("search");
    let paths: Vec<&str> = search
        .and_then(|s| s.get("paths"))
        .and_then(Node::items)
        .unwrap_or_default()
        .iter()
        .map(|p| p.string("path").unwrap_or_default())
        .chain(search.and_then(|s| s.string("path")))
        .collect();
    if paths.is_empty() {
        bail!("API origin definitions must declare absolute search paths");
    }
    let mut expected = Vec::new();
    for path in paths {
        let lower = path.to_ascii_lowercase();
        let Some(scheme) = ["https", "http"]
            .into_iter()
            .find(|s| lower.starts_with(&format!("{s}://")))
        else {
            bail!("API origin definitions must declare absolute search paths");
        };
        let authority = path[scheme.len() + 3..]
            .split('/')
            .next()
            .unwrap_or_default();
        let host = match authority
            .strip_prefix("{{")
            .and_then(|a| a.strip_suffix("}}"))
            .and_then(|a| a.trim().strip_prefix(".Config."))
        {
            Some(name) => settings
                .get(name)
                .and_then(Value::as_str)
                .or_else(|| {
                    r.get("settings")
                        .and_then(Node::items)
                        .unwrap_or_default()
                        .iter()
                        .find(|s| s.string("name") == Some(name))
                        .and_then(|s| s.string("default"))
                })
                .unwrap_or_default(),
            None if !authority.contains("{{") => authority,
            None => bail!("Unsupported API host template"),
        };
        if host.is_empty() {
            return Err(invalid());
        }
        expected.push(
            url::Url::parse(&format!("{scheme}://{host}/"))
                .map_err(|_| invalid())?
                .origin(),
        );
    }
    let ctx = plan.context(settings, &SearchQuery::default(), chrono::Utc::now());
    let requests = plan.build_requests(&ctx).map_err(|_| invalid())?;
    let mut origin: Option<url::Url> = None;
    for request in requests {
        let mut u = url::Url::parse(&request.url).map_err(|_| invalid())?;
        if !u.username().is_empty() || u.password().is_some() || !expected.contains(&u.origin()) {
            return Err(invalid());
        }
        u.set_path("/");
        u.set_query(None);
        u.set_fragment(None);
        match &origin {
            Some(o) if o.origin() != u.origin() => {
                bail!("API origin definitions must search a single API host")
            }
            Some(_) => {}
            None => origin = Some(u),
        }
    }
    Ok(origin.filter(|o| o.origin() != site.origin()))
}
fn schema(r: &Node) -> Vec<Value> {
    r.get("settings").and_then(Node::items).unwrap_or_default().iter().map(|s|{
  let source_type=s.string("type").unwrap_or("text");
  let typ=if ["info_useragent","info_category_8000"].contains(&source_type) {"info"} else {source_type};
  let options:Vec<_>=s.get("options").and_then(Node::pairs).unwrap_or_default().iter().map(|(k,v)|json!({"value":k.text().unwrap_or(""),"label":v.text().unwrap_or("")})).collect();
  let default=match typ {"checkbox"=>json!(s.string("default")==Some("true")),"multi-select"=>json!(s.get("defaults").and_then(Node::items).unwrap_or_default().iter().filter_map(Node::text).collect::<Vec<_>>()),_=>json!(s.string("default").unwrap_or(""))};
  json!({"name":s.string("name").unwrap_or(""),"label":s.string("label").unwrap_or("Information"),"type":typ,"secret":(["text","password"].contains(&typ)),"default":if source_type=="info_useragent" {json!("Enter the User-Agent request header from the browser session that supplied this cookie. It is sent on login checks, searches and downloads.")} else if source_type=="info_category_8000" {json!("This source groups results under Other (8000). Choose Other to include its results in category-filtered searches.")} else {default},"options":options})
 }).collect()
}
fn modes(r: &Node) -> BTreeMap<String, Vec<String>> {
    r.get("caps")
        .and_then(|c| c.get("modes"))
        .and_then(Node::pairs)
        .unwrap_or_default()
        .iter()
        .map(|(k, v)| {
            (
                k.text().unwrap_or("").into(),
                v.items()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Node::text)
                    .map(str::to_owned)
                    .collect(),
            )
        })
        .collect()
}
pub fn catalog() -> &'static Vec<Value> {
    static CATALOG: OnceLock<Vec<Value>> = OnceLock::new();
    CATALOG.get_or_init(||SOURCES.iter().map(|(id,_)|{
  let r=root(id).expect("Pinned valid definition");let compiled=ready(&r);let categories=compiled.as_ref().map(|p|p.supported_categories()).unwrap_or_default();let reason=compiled.err().map(|e|e.to_string());
  let hash=CHECKSUMS.lines().find_map(|line|line.split_once("  ").filter(|(_,path)|*path==format!("definitions/{id}.yml")).map(|(hash,_)|hash));
  json!({"id":id,"name":r.string("name"),"description":r.string("description"),"language":r.string("language"),"type":r.string("type"),"kind":"cardigann","url":r.get("links").and_then(Node::items).and_then(|v|v.first()).and_then(Node::text),"ready":reason.is_none(),"blocked_reason":reason,"categories":categories,"caps":modes(&r),"settings":schema(&r),"sha256":hash,"provenance":PROVENANCE})
 }).collect())
}
pub fn id(i: &Indexer) -> Result<&str> {
    i.definition
        .as_ref()
        .and_then(|d| d.get("builtin_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Select a bundled definition"))
}
pub fn public_definition(i: &Indexer) -> Option<Value> {
    if i.kind != "cardigann" {
        return None;
    }
    let mut d = i.definition.clone()?;
    let r = root(id(i).ok()?).ok()?;
    for s in schema(&r) {
        if s["secret"] == true {
            if let Some(obj) = d.get_mut("settings").and_then(Value::as_object_mut) {
                obj.remove(s["name"].as_str()?);
            }
        }
    }
    Some(d)
}
/// True only for two valid HTTP(S) URLs sharing scheme, host and effective port.
pub fn same_origin(a: &str, b: &str) -> bool {
    let origin = |s: &str| {
        url::Url::parse(s)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
            .map(|u| u.origin())
    };
    matches!((origin(a), origin(b)), (Some(a), Some(b)) if a == b)
}
/// Saved secrets are implicitly carried forward only within the same origin.
pub fn preserve_secrets(new: &mut Indexer, old: &Indexer) {
    if new.kind != "cardigann" || id(new).ok() != id(old).ok() || !same_origin(&new.url, &old.url) {
        return;
    }
    let Ok(r) = root(id(old).unwrap_or("")) else {
        return;
    };
    if let Some(d) = new.definition.as_mut().and_then(Value::as_object_mut) {
        d.entry("settings").or_insert_with(|| json!({}));
    }
    for s in schema(&r) {
        if s["secret"] != true {
            continue;
        }
        let name = s["name"].as_str().unwrap_or("");
        if let Some(settings) = new
            .definition
            .as_mut()
            .and_then(|d| d.get_mut("settings"))
            .and_then(Value::as_object_mut)
        {
            if settings.get(name).is_none_or(|v| v.as_str() == Some("")) {
                if let Some(v) = old
                    .definition
                    .as_ref()
                    .and_then(|d| d.get("settings"))
                    .and_then(|s| s.get(name))
                {
                    settings.insert(name.into(), v.clone());
                }
            }
        }
    }
}
pub struct Runtime {
    pub plan: Arc<Plan>,
    pub settings: BTreeMap<String, Value>,
    pub delay_ms: u64,
    pub modes: BTreeMap<String, Vec<String>>,
    pub empty_html_container: Option<String>,
    /// Detail-page magnet resolution for ticketed links; `None` downloads them directly.
    pub download: Option<DetailMagnet>,
    /// The admitted API origin this source searches besides its site ([`api_origin`]).
    pub api_origin: Option<url::Url>,
}
impl Runtime {
    pub fn compile(i: &Indexer) -> Result<Self> {
        Self::compile_gated(i, ready)
    }
    /// Test fixtures only: every check except the [`READY_IDS`] gate.
    #[cfg(test)]
    pub(crate) fn compile_unready(i: &Indexer) -> Result<Self> {
        Self::compile_gated(i, eligible)
    }
    fn compile_gated(i: &Indexer, gate: fn(&Node) -> Result<Plan>) -> Result<Self> {
        if !i.api_key.is_empty() {
            bail!("Use bundled preset settings; generic credentials are unsupported");
        }
        // Validate public challenge cookies and supplied-cookie login sessions before saving.
        crate::builtin_runtime::Source::new(i, None)?;
        let mut r = root(id(i)?)?;
        gate(&r)?;
        let base = url::Url::parse(&i.url).map_err(|_| anyhow!("Invalid mirror URL"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            bail!("Mirror must be HTTP(S) without credentials, query or fragment");
        }
        let envelope = i
            .definition
            .as_ref()
            .unwrap()
            .as_object()
            .ok_or_else(|| anyhow!("Invalid definition envelope"))?;
        if envelope
            .keys()
            .any(|k| !["builtin_id", "settings"].contains(&k.as_str()))
        {
            bail!("Unknown bundled configuration field");
        }
        let mut settings = BTreeMap::new();
        let sc = schema(&r);
        if let Some(values) = envelope.get("settings") {
            for (k, v) in values
                .as_object()
                .ok_or_else(|| anyhow!("Settings must be an object"))?
            {
                let s = sc
                    .iter()
                    .find(|s| s["name"] == k.as_str())
                    .ok_or_else(|| anyhow!("Unknown bundled setting"))?;
                let valid = match s["type"].as_str().unwrap_or("") {
                    "text" | "password" => v
                        .as_str()
                        .is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control)),
                    "checkbox" => v.is_boolean(),
                    "select" => s["options"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|o| o["value"] == *v),
                    "multi-select" => v.as_array().is_some_and(|vs| {
                        vs.len() <= 128
                            && vs.iter().all(|v| {
                                s["options"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .any(|o| o["value"] == *v)
                            })
                    }),
                    _ => false,
                };
                if !valid {
                    bail!("Invalid bundled setting value");
                }
                settings.insert(k.clone(), v.clone());
            }
        }
        let delay_ms = request_delay_ms(&r, i.request_delay_ms)?;
        // Clone only the wrapper AST: immutable interpreter APIs and source files remain unchanged.
        if let cardigann::Value::Mapping(pairs) = &mut r.value {
            for (k, v) in pairs {
                if k.text() == Some("links") {
                    let mut link = v
                        .items()
                        .and_then(|v| v.first())
                        .cloned()
                        .ok_or_else(|| anyhow!("Missing bundled link"))?;
                    if let cardigann::Value::Scalar(s) = &mut link.value {
                        s.text = base.to_string();
                    }
                    v.value = cardigann::Value::Sequence(vec![link]);
                }
            }
        }
        let plan = gate(&r)?;
        plan.check_settings(&settings)
            .map_err(|_| anyhow!("Invalid bundled setting value"))?;
        let api_origin = api_origin(&r, &plan, &settings, &base)?;
        // Resolve the settings of the admitted DetailMagnet shape. Any other admitted
        // download block is general selector resolution, see [`download_selectors`].
        let download = if detail_magnet_shape(&r) {
            let [primary, fallback] = LINK_SETTINGS.map(|name| {
                settings
                    .get(name)
                    .and_then(Value::as_str)
                    .or_else(|| link_setting(&r, name).and_then(|s| s.string("default")))
                    .unwrap_or_default()
            });
            if primary != "magnet:" {
                bail!("Only the magnet primary download link is supported; iTorrents downloads are unavailable");
            }
            Some(DetailMagnet {
                fallback: if fallback == "magnet:" {
                    Fallback::Magnet
                } else {
                    Fallback::ITorrents
                },
            })
        } else {
            None
        };
        Ok(Self {
            plan: Arc::new(plan),
            settings,
            delay_ms,
            modes: modes(&r),
            empty_html_container: match id(i)? {
                "acgrip" => Some("tbody".into()),
                "anirena" => Some("table.tl-table".into()),
                "shanaproject" => Some("div.grid_12".into()),
                "u3c3" => Some("table.torrent-list".into()),
                "torrent-pirat" => Some("table.tablebg:nth-of-type(2)".into()),
                "xxxtor" | "torrentbyte" | "nipponsei" | "sukebeinyaasi" | "extratorrent-st"
                | "rutracker-ru" | "sexypics" => Some("table".into()),
                "kickasstorrents-to" | "kickasstorrents-ws" => Some("table.data".into()),
                "mikan" => Some("table.table-striped".into()),
                "newstudio" => Some("table.well".into()),
                "linuxtracker" => Some("table.torrent-table".into()),
                "opensharing" => Some("table.tablebg".into()),
                "btetree" => Some("table[bgcolor=\"#CCCCCC\"] tbody".into()),
                "tokyotosho" => Some("table.listing".into()),
                "uindex" => Some("table.sr-table".into()),
                "bigfangroup" => Some("tbody#highlighted".into()),
                "megapeer" | "torrentby" => Some("table".into()),
                "mypornclub" => Some("div.torrents_list".into()),
                // DataLife Engine (its /engine/download.php links) wraps results here.
                "pctorrent" => Some("div#dle-content".into()),
                "ehentai" => Some("table.itg".into()),
                "plugintorrent" => Some("main".into()),
                "anibt" => Some("channel".into()),
                "nekobt" => Some("channel".into()),
                "pandacd" => Some("channel".into()),
                "pornotorrent" => Some("channel".into()),
                "0magnet" => Some("table".into()),
                "52bt" => Some("main".into()),
                "anisource" => Some("div#content".into()),
                "blueroms" => Some("div.row".into()),
                "btdirectory" => Some("main".into()),
                "crackingpatching" => Some("main".into()),
                "freejavtorrent" => Some("main".into()),
                "mactorrentsdownload" => Some("main".into()),
                "magnetcat" => Some("main".into()),
                "mixtapetorrent" => Some("dl.search-results".into()),
                "nyaasi" => Some("table".into()),
                "onejav" => Some("div.columns".into()),
                "pornrips" => Some("main".into()),
                "pornxlab" => Some("main".into()),
                // DataLife Engine search (do=search&subaction=search), as pctorrent.
                "byrutor" => Some("div#dle-content".into()),
                "rutor" => Some("div#index".into()),
                "noname-club" => Some("table.forumline.tablesorter".into()),
                // The scopes the pinned row selectors name.
                "torrentcore" => Some("div.content".into()),
                "damagnet" => Some("table.table-striped".into()),
                "limetorrents" => Some("table.table2".into()),
                "torrentdownloads" => Some("div.inner_container".into()),
                "magnetdownload" => Some("table".into()),
                // DataLife Engine search, as byrutor.
                "catorrent" | "skidrowrepack" => Some("div#dle-content".into()),
                "ebookbay" => Some("div#content".into()),
                // Torznab XML API sources (`login: get`): results sit in the RSS channel.
                "binarybay" => Some("channel".into()),
                "bluetorrents" => Some("channel".into()),
                "fileiplay" => Some("channel".into()),
                "periodical" => Some("channel".into()),
                "retrotoon" => Some("channel".into()),
                "shadowbit" => Some("channel".into()),
                "scenetime-api" => Some("channel".into()),
                "world-of-tomorrow-api" => Some("channel".into()),
                // RSS-shaped XML API (`rows: item`).
                "dasunerwartete-api" => Some("channel".into()),
                "showrss" => Some("channel".into()),
                "sosulki" => Some("table".into()),
                "torrentkitty" => Some("table#archiveResult".into()),
                "torrentdownload" => Some("table.table2".into()),
                "torrentgalaxyclone" => Some("div.tgxtable".into()),
                "torrent9" => Some("table.table-striped".into()),
                "torrenttip" => Some("ul.page-list".into()),
                "torrentsome" => Some("div.topic-list".into()),
                "vsthouse" => Some("div#content".into()),
                "vstorrent" => Some("main".into()),
                "vsttorrents" => Some("main".into()),
                "xxxclub" => Some("div.browsetableinside".into()),
                "rintornet" => Some("div#dle".into()),
                "bitmagnet" => Some("channel".into()),
                "filemood" => Some("table".into()),
                "world-torrent" => Some("table.table".into()),
                "bitagent" => Some("channel".into()),
                "stellarwinds" => Some("channel".into()),
                "torrentclaw" => Some("channel".into()),
                "astratorrent" => Some("channel".into()),
                "c411" => Some("channel".into()),
                "crazyspirits-api" => Some("channel".into()),
                "draupnirr" => Some("channel".into()),
                "memphis" => Some("channel".into()),
                "nostradamus" => Some("channel".into()),
                "tr4ker" => Some("channel".into()),
                "torrentproject2" => Some("div#similarfiles".into()),
                // Torznab XML from the API origin (docs/api-origins.md).
                "animetosho-xyz" => Some("channel".into()),
                _ => None,
            },
            download,
            api_origin,
        })
    }
    pub fn validate_query(&self, q: &SearchQuery) -> Result<()> {
        let mode = match q.get("t") {
            "" | "search" => "search",
            "tvsearch" => "tv-search",
            "movie" => "movie-search",
            "music" => "music-search",
            "book" => "book-search",
            _ => bail!("Unsupported search mode"),
        };
        let supported = self
            .modes
            .get(mode)
            .ok_or_else(|| anyhow!("Search mode unsupported by this definition"))?;
        for key in [
            "imdbid",
            "tmdbid",
            "tvdbid",
            "tvmazeid",
            "rid",
            "traktid",
            "doubanid",
            "season",
            "ep",
            "artist",
            "album",
            "author",
            "title",
            "publisher",
        ] {
            if !q.get(key).is_empty() && !supported.iter().any(|p| p == key) {
                bail!("Query parameter unsupported by this definition");
            }
        }
        Ok(())
    }
}
pub fn source_categories(i: &Indexer) -> Vec<u32> {
    if i.kind != "cardigann" {
        return i.categories.clone();
    }
    // The catalog entry's categories, computed for this one definition: building the
    // whole catalog compiles all 583 definitions, which once delayed the first search.
    static CATEGORIES: OnceLock<Vec<OnceLock<Vec<u32>>>> = OnceLock::new();
    let Some(n) = id(i)
        .ok()
        .and_then(|id| SOURCES.binary_search_by_key(&id, |(id, _)| *id).ok())
    else {
        return Vec::new();
    };
    let cells = CATEGORIES.get_or_init(|| SOURCES.iter().map(|_| OnceLock::new()).collect());
    cells[n]
        .get_or_init(|| {
            root(SOURCES[n].0)
                .ok()
                .and_then(|r| ready(&r).ok())
                .map(|p| p.supported_categories())
                .unwrap_or_default()
        })
        .clone()
}
// Capabilities belong to the immutable embedded definition, not the configured
// mirror, credentials or result cache. Parse each requested definition once;
// warming one source must not compile the entire 583-entry catalog.
type Modes = BTreeMap<String, Vec<String>>;
fn cached_modes(id: &str) -> Option<&'static Modes> {
    static MODES: OnceLock<Vec<OnceLock<Modes>>> = OnceLock::new();
    let n = SOURCES.binary_search_by_key(&id, |(id, _)| *id).ok()?;
    let cells = MODES.get_or_init(|| SOURCES.iter().map(|_| OnceLock::new()).collect());
    Some(cells[n].get_or_init(|| root(id).map(|r| modes(&r)).unwrap_or_default()))
}
pub fn source_modes(i: &Indexer) -> BTreeMap<String, Vec<String>> {
    if i.kind == "cardigann" {
        return id(i)
            .ok()
            .and_then(cached_modes)
            .cloned()
            .unwrap_or_default();
    }
    let mut modes: BTreeMap<String, Vec<String>> = [
        ("search", vec!["q"]),
        ("tv-search", vec!["q", "season", "ep"]),
        ("movie-search", vec!["q"]),
        ("music-search", vec!["q", "artist", "album"]),
        ("book-search", vec!["q", "title", "author", "publisher"]),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into_iter().map(str::to_owned).collect()))
    .collect();
    if i.kind == "unit3d" {
        for mode in ["tv-search", "movie-search"] {
            modes.get_mut(mode).unwrap().extend(
                ["imdbid", "tmdbid", "tvdbid"]
                    .into_iter()
                    .map(str::to_owned),
            );
        }
    }
    modes
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pinned_source(id: &str) -> &'static str {
        SOURCES.iter().find(|(i, _)| *i == id).unwrap().1
    }
    fn parsed(source: &str) -> Node {
        cardigann::parse(source, cardigann::Limits::default()).unwrap()
    }
    #[test]
    fn immutable_capabilities_match_every_pinned_definition_and_configured_source() {
        for (id, _) in SOURCES {
            let expected = modes(&root(id).unwrap());
            let cached = cached_modes(id).unwrap();
            assert_eq!(*cached, expected, "{id}");
            assert!(std::ptr::eq(cached, cached_modes(id).unwrap()));
            let mut source = Indexer {
                kind: "cardigann".into(),
                definition: Some(json!({"builtin_id":id,"settings":{}})),
                ..Default::default()
            };
            assert_eq!(source_modes(&source), expected, "{id}");
            source.url = "https://changed.example/".into();
            source.definition.as_mut().unwrap()["settings"] = json!({"uploader":"private fixture"});
            let mut returned = source_modes(&source);
            assert_eq!(returned, expected, "{id}");
            returned.clear();
            assert_eq!(source_modes(&source), expected, "{id}");
        }
        assert!(cached_modes("missing-fixture-id").is_none());
        for definition in [None, Some(json!({"builtin_id":"missing-fixture-id"}))] {
            assert!(source_modes(&Indexer {
                kind: "cardigann".into(),
                definition,
                ..Default::default()
            })
            .is_empty());
        }
        // Concurrent callers share the immutable entry; returned maps remain independent.
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| cached_modes("1337x").unwrap()))
                .collect();
            for job in jobs {
                assert!(std::ptr::eq(
                    job.join().unwrap(),
                    cached_modes("1337x").unwrap()
                ));
            }
        });
    }
    #[test]
    fn authenticated_and_public_delay_policy_keeps_pinned_minimum_and_bounds() {
        assert_eq!(
            request_delay_ms(&parsed("requestDelay: 2"), 1000).unwrap(),
            2000
        );
        assert_eq!(
            request_delay_ms(&parsed("requestDelay: 0.0001"), 0).unwrap(),
            1
        );
        assert_eq!(request_delay_ms(&parsed("id: source"), 3000).unwrap(), 3000);
        assert_eq!(
            request_delay_ms(&parsed("requestDelay: 2"), 5000).unwrap(),
            5000
        );
        for raw in ["-1", "61", ".nan", "NaN", "Infinity", "[]", "{}"] {
            assert!(request_delay_ms(&parsed(&format!("requestDelay: {raw}")), 0).is_err());
        }
        assert!(request_delay_ms(&parsed("id: source"), 60001).is_err());
    }
    #[test]
    fn only_the_unchanged_1337x_download_block_is_admitted() {
        let admitted: Vec<_> = SOURCES
            .iter()
            .filter(|(id, _)| root(id).is_ok_and(|r| detail_magnet_shape(&r)))
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(admitted, ["1337x"]);
        assert!(READY_IDS.contains(&"1337x"));
        let reason = |id: &str| {
            catalog().iter().find(|e| e["id"] == id).unwrap()["blocked_reason"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert!(catalog().iter().find(|e| e["id"] == "1337x").unwrap()["blocked_reason"].is_null());
        // The same settings and rewrite with other templated selectors are general
        // download.selectors resolution (limetorrents, fixture-validated), never the
        // 1337x policy. A plain magnet selector is eligible but not ready without fixtures.
        assert!(!detail_magnet_shape(&root("limetorrents").unwrap()));
        assert!(catalog()
            .iter()
            .find(|e| e["id"] == "limetorrents")
            .unwrap()["blocked_reason"]
            .is_null());
        assert!(eligible(&root("polishtracker-api").unwrap()).is_ok());
        assert_eq!(
            reason("polishtracker-api"),
            "Runtime integration has not been fixture-validated for this definition"
        );
        assert!(eligible(&root("1337x").unwrap()).is_ok());
    }
    #[test]
    fn altered_1337x_download_blocks_leave_the_detail_magnet_policy() {
        let original = pinned_source("1337x");
        for (from, to) in [
            (
                r#"- selector: ul li a[href*="{{ .Config.primarydownloadlink }}"]"#,
                r#"- selector: a[href*="{{ .Config.primarydownloadlink }}"]"#,
            ),
            (
                r#"ul li a[href*="{{ .Config.fallbackdownloadlink }}"]"#,
                r#"ul li a[href*="{{ .Config.primarydownloadlink }}"]"#,
            ),
            (
                r#"args: ["http://itorrents.org/", "https://itorrents.net/"]"#,
                r#"args: ["http://itorrents.org/", "https://attacker.test/"]"#,
            ),
            ("      attribute: href\n", "      attribute: data-href\n"),
            ("        - name: replace\n", "        - name: re_replace\n"),
            ("download:\n", "download:\n  method: post\n"),
            ("download:\n", "download:\n  headers:\n    X-Fixture: [\"1\"]\n"),
            (
                "search:\n  paths:\n",
                "search:\n  headers:\n    X-Fixture: [\"1\"]\n  paths:\n",
            ),
            (
                "  selectors:\n",
                "  selectors:\n    - selector: ul li a[href*=\"magnet:\"]\n      attribute: href\n      filters:\n        - name: replace\n          args: [\"http://itorrents.org/\", \"https://itorrents.net/\"]\n",
            ),
            (
                "      \"magnet:\": magnet\n",
                "      \"magnet:\": magnet\n      \"http:\": direct\n",
            ),
            (
                "    type: select\n    label: Primary",
                "    type: text\n    label: Primary",
            ),
        ] {
            assert!(original.contains(from), "{from}");
            let r = parsed(&original.replacen(from, to, 1));
            assert!(!detail_magnet_shape(&r), "{to}");
            // Off the 1337x policy, the general same-origin resolver takes over
            // (docs/download-selectors.md); download method and headers stay blocked.
            let general = crate::builtin_runtime::download::DownloadSelectors::compile(&r).is_ok();
            assert_eq!(general, !to.starts_with("download:\n  "), "{to}");
            let e = eligible(&r).err().map(|e| e.to_string()).unwrap_or_default();
            assert!(general == e.is_empty() && (general || e.starts_with("$.")), "{to}: {e}");
        }
        // A detail-page download removed entirely leaves no policy to apply.
        let start = original.find("download:\n").unwrap();
        let end = original.find("search:\n").unwrap();
        let r = parsed(&format!("{}{}", &original[..start], &original[end..]));
        assert!(!detail_magnet_shape(&r));
        assert!(eligible(&r).is_ok());
    }
    fn stock_1337x(settings: Value) -> Indexer {
        Indexer {
            kind: "cardigann".into(),
            url: "https://1337x.to/".into(),
            definition: Some(json!({"builtin_id":"1337x","settings":settings})),
            ..Default::default()
        }
    }
    #[test]
    fn configured_link_settings_select_the_policy_before_any_io() {
        let owner = json!({"uploader":"","primarydownloadlink":"magnet:",
            "fallbackdownloadlink":"//itorrents.","sort":"time","type":"desc","disablesort":false});
        let policy =
            |settings| Runtime::compile_unready(&stock_1337x(settings)).map(|r| r.download);
        for settings in [owner.clone(), json!({})] {
            assert_eq!(
                policy(settings).unwrap(),
                Some(DetailMagnet {
                    fallback: Fallback::ITorrents
                })
            );
        }
        assert_eq!(
            policy(json!({"fallbackdownloadlink":"magnet:"})).unwrap(),
            Some(DetailMagnet {
                fallback: Fallback::Magnet
            })
        );
        for primary in ["//itorrents.", "magnet", "https:", ""] {
            assert!(
                policy(json!({"primarydownloadlink":primary})).is_err(),
                "{primary}"
            );
        }
        assert!(policy(json!({"primarydownloadlink":"//itorrents."}))
            .unwrap_err()
            .to_string()
            .contains("Only the magnet primary"));
        assert_eq!(
            Runtime::compile(&stock_1337x(owner)).unwrap().download,
            Some(DetailMagnet {
                fallback: Fallback::ITorrents
            })
        );
        let mut acgrip = stock_1337x(json!({}));
        acgrip.definition = Some(json!({"builtin_id":"acgrip"}));
        assert_eq!(Runtime::compile(&acgrip).unwrap().download, None);
    }
    #[test]
    fn compilation_does_not_authorize_an_unreviewed_runtime_id() {
        let mut r = root("acgrip").unwrap();
        if let cardigann::Value::Mapping(pairs) = &mut r.value {
            for (k, v) in pairs {
                if k.text() == Some("id") {
                    if let cardigann::Value::Scalar(s) = &mut v.value {
                        s.text = "unreviewed-acgrip".into();
                    }
                }
            }
        }
        assert!(Plan::compile(&r).is_ok());
        assert!(ready(&r)
            .err()
            .unwrap()
            .to_string()
            .contains("fixture-validated"));
    }
    #[test]
    fn single_byte_plans_are_eligible_but_need_fixture_admission() {
        // The definition charset no longer blocks eligibility; readiness still needs an
        // explicit fixture-backed ID and every login family keeps its own gate.
        let r = cardigann::parse("id: acgrip\nname: Latin\ndescription: Synthetic\nlanguage: en-US\ntype: public\nencoding: ISO-8859-1\nlinks: [https://example.test/]\ncaps:\n  categories: {1: Other}\n  modes: {search: [q]}\nsettings: []\nsearch:\n  path: search\n  rows: {selector: article}\n  fields:\n    category: {text: 1}\n    size: {text: 0}\n    seeders: {text: 0}\n    title: {text: x}\n    download: {text: /d}\n", cardigann::Limits::default()).unwrap();
        let plan = Plan::compile(&r).unwrap();
        assert_eq!(
            plan.charset(),
            crate::cardigann_exec::request::Charset::Latin1
        );
        assert!(plan.blockers().is_empty());
        assert!(eligible(&r).is_ok());
        // windows-1255, windows-874/TIS-620 and Latin-1 urldecode are fixture-admitted
        // since 2026-11-01 (docs/legacy-encodings.md).
        for id in ["aftershock", "fuzer", "siambit", "unlimitz"] {
            assert!(ready(&root(id).unwrap()).is_ok(), "{id}");
        }
    }
}
