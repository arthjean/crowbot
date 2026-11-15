//! The pinned runtimes. A hash here is the SHA-256 of the file its publisher
//! serves at the address next to it: the value the publisher gives for the
//! Claude agent and for Bun (GitHub release digests), and the value computed
//! from one download on 2026-11-15 for the others.

use std::collections::BTreeMap;

use super::{Artifact, RuntimeSpec, Unpack};

pub const PLATFORMS: [&str; 6] = [
    "windows-x64",
    "windows-arm64",
    "linux-x64",
    "linux-arm64",
    "macos-x64",
    "macos-arm64",
];

const CLAUDE_VERSION: &str = "0.107.0";
const CODEX_ADAPTER_VERSION: &str = "2.6.0";
const CODEX_ADAPTER_SHA256: &str =
    "8d754b69e58ec0b3820039cefe0987cb059b2ef439d20415a5e875056abaebe3";
const CODEX_ADAPTER_BYTES: u64 = 407881;
const CODEX_VERSION: &str = "0.166.0";
const BUN_VERSION: &str = "1.4.4";
const GROK_VERSION: &str = "1.0.77";

fn platform() -> Option<&'static str> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "linux" if cfg!(target_env = "gnu") => "linux",
        "macos" => "macos",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    PLATFORMS
        .into_iter()
        .find(|platform| *platform == format!("{os}-{arch}"))
}

pub fn spec(provider: &str) -> Option<RuntimeSpec> {
    spec_for(provider, platform()?)
}

pub fn spec_for(provider: &str, platform: &str) -> Option<RuntimeSpec> {
    match provider {
        "claude" => claude(platform),
        "codex" => codex(platform),
        "grok" => grok(platform),
        _ => None,
    }
}

fn exe(platform: &str) -> &'static str {
    if platform.starts_with("windows") {
        ".exe"
    } else {
        ""
    }
}

fn artifact(url: String, sha256: &str, bytes: u64, unpack: Unpack, into: &str) -> Artifact {
    Artifact {
        url,
        sha256: sha256.to_owned(),
        bytes,
        unpack,
        into: into.to_owned(),
        file: None,
    }
}

fn claude(platform: &str) -> Option<RuntimeSpec> {
    let (file, unpack, sha256, bytes) = match platform {
        "windows-x64" => (
            "windows-x64.zip",
            Unpack::Zip,
            "bb406522d6341b3b9aa1b9bfcaa996ddf8cd53b91512f92b111a8414a049775f",
            228_289_285,
        ),
        "windows-arm64" => (
            "windows-arm64.zip",
            Unpack::Zip,
            "92be1f854b13d4ceb133dbec69bf3fbcdb86d9807a583d45cbd6cddf29f76307",
            217_006_708,
        ),
        "linux-x64" => (
            "linux-x64.tar.gz",
            Unpack::TarGz,
            "768f0c910587b8c2a8c3a96aedd5a8876128277f6d633bbcd52092b229e056a9",
            218_082_028,
        ),
        "linux-arm64" => (
            "linux-arm64.tar.gz",
            Unpack::TarGz,
            "b63b7e5164a0585bdc4837943238170f156daed0d3a9effb093f7d93da206d7d",
            217_692_429,
        ),
        "macos-x64" => (
            "darwin-x64.zip",
            Unpack::Zip,
            "79d46ef951ad359776022c2cf3f85bb17cf16c1aedee68e8236370a6b15f1367",
            205_591_945,
        ),
        "macos-arm64" => (
            "darwin-arm64.zip",
            Unpack::Zip,
            "ba106984d2d89b4e260ac0648e354c924a1a1545392929b1e27700615d7ba9b3",
            195_217_810,
        ),
        _ => return None,
    };
    let url = format!(
        "https://github.com/agentclientprotocol/claude-agent-acp/releases/download/v{CLAUDE_VERSION}/claude-agent-acp-{file}"
    );
    Some(RuntimeSpec {
        name: "claude-agent-acp".to_owned(),
        version: CLAUDE_VERSION.to_owned(),
        artifacts: vec![artifact(url, sha256, bytes, unpack, "")],
        program: format!("claude-agent-acp{}", exe(platform)),
        args: Vec::new(),
        env: BTreeMap::new(),
    })
}

fn codex(platform: &str) -> Option<RuntimeSpec> {
    let (bun, bun_sha256, bun_bytes, cli, cli_sha256, cli_bytes) = match platform {
        "windows-x64" => (
            "windows-x64-baseline",
            "0f20ce6674d4f2dbcc8c5d59fdf99f7ba45f8c3b367e0f5781cbfa30a1c78d99",
            54_731_686,
            "win32-x64",
            "47d74aed875a3c01315f042152329ae09b0bd9d26a3285802d1c85aeadce083e",
            180_706_795,
        ),
        "windows-arm64" => (
            "windows-aarch64",
            "2d57a0eec0cb44d95c918c3f9734f60455a1306ae5085d76fc68f44e3ee30d09",
            49_392_935,
            "win32-arm64",
            "0af3dbf9675d199026e75d0c01ccbd231ece959ec0ff8207a5db8b4d625e0d00",
            168_971_160,
        ),
        "linux-x64" => (
            "linux-x64-baseline",
            "e0446eb55995772eb9894bebae9a6be368d985a8b9271ee124486f9946eedd76",
            51_794_559,
            "linux-x64",
            "cbc00a26d3d0e2ceefc4be8891f8a81ef1e349b3b175bc0ae308762bc001fd93",
            172_325_449,
        ),
        "linux-arm64" => (
            "linux-aarch64",
            "baa6bb36f51d6e58561e02bbea825038a9df454f673d6d776e8bbf86f8c7850e",
            51_644_673,
            "linux-arm64",
            "9044e840397864fc6c539d13719b25fb26abb35d5353366cbabf7b4555406acd",
            164_402_913,
        ),
        "macos-x64" => (
            "darwin-x64-baseline",
            "d4a155adfbfd09155eb3ba05c9e3a544f39297d7a68616d3c7759f67ab5c47dc",
            43_442_607,
            "darwin-x64",
            "588aad29a4779badc894daeb2e43757ed01d213366fd931fd9a5ec47f7e31e14",
            156_633_428,
        ),
        "macos-arm64" => (
            "darwin-aarch64",
            "c240d7f0ccb90c263a849fcdb09b7ee62630d921348ed6aafda86b9d577bc510",
            40_323_673,
            "darwin-arm64",
            "181d2eda8addbeca0b689c996df207293947928bdd9c88a097837ae1c7e69ca5",
            147_077_955,
        ),
        _ => return None,
    };
    let exe = exe(platform);
    Some(RuntimeSpec {
        name: "codex-acp".to_owned(),
        version: format!("{CODEX_ADAPTER_VERSION}-codex-{CODEX_VERSION}-bun-{BUN_VERSION}"),
        artifacts: vec![
            artifact(
                format!(
                    "https://github.com/oven-sh/bun/releases/download/bun-v{BUN_VERSION}/bun-{bun}.zip"
                ),
                bun_sha256,
                bun_bytes,
                Unpack::Zip,
                "bun",
            ),
            artifact(
                format!(
                    "https://registry.npmjs.org/@agentclientprotocol/codex-acp/-/codex-acp-{CODEX_ADAPTER_VERSION}.tgz"
                ),
                CODEX_ADAPTER_SHA256,
                CODEX_ADAPTER_BYTES,
                Unpack::TarGz,
                "adapter",
            ),
            artifact(
                format!(
                    "https://registry.npmjs.org/@openai/codex/-/codex-{CODEX_VERSION}-{cli}.tgz"
                ),
                cli_sha256,
                cli_bytes,
                Unpack::TarGz,
                "codex",
            ),
        ],
        program: format!("bun/*/bun{exe}"),
        args: vec!["{root}/adapter/package/dist/index.js".to_owned()],
        env: BTreeMap::from([(
            "CODEX_PATH".to_owned(),
            format!("{{root}}/codex/package/vendor/*/bin/codex{exe}"),
        )]),
    })
}

fn grok(platform: &str) -> Option<RuntimeSpec> {
    let (name, sha256, bytes) = match platform {
        "windows-x64" => (
            "windows-x86_64.exe",
            "3cb3341896eaf01e435224c942e39f7fd2f6fbb74eb6d0bd8639c90e8a34c55b",
            71_906_002,
        ),
        "windows-arm64" => (
            "windows-aarch64.exe",
            "84cb23f39d8a75779ea721ea6e79a09f5ddfa9eb0afcf15308e6bd72106aaaf3",
            67_999_605,
        ),
        "linux-x64" => (
            "linux-x86_64",
            "243a669421d0af2c3a56b4283601d8c22815312224feb38a0059396659926250",
            78_994_504,
        ),
        "linux-arm64" => (
            "linux-aarch64",
            "ad0bdf44d84197f4a746d7f3fb949724c63023b89baefc54a4132303da7bff87",
            73_931_213,
        ),
        "macos-x64" => (
            "macos-x86_64",
            "eef14024e63da5d7627d8cecaa3c1b0151dfc113cebc4609739ad2a000a83cc6",
            80_565_254,
        ),
        "macos-arm64" => (
            "macos-aarch64",
            "4299490f441e02628db9ac3f24a6004aef87bc4bde32af3f1dac372f13994704",
            74_824_527,
        ),
        _ => return None,
    };
    let program = format!("grok{}", exe(platform));
    Some(RuntimeSpec {
        name: "grok".to_owned(),
        version: GROK_VERSION.to_owned(),
        artifacts: vec![Artifact {
            file: Some(program.clone()),
            ..artifact(
                format!("https://x.ai/cli/grok-{GROK_VERSION}-{name}.gz"),
                sha256,
                bytes,
                Unpack::Gz,
                "",
            )
        }],
        program,
        args: Vec::new(),
        env: BTreeMap::new(),
    })
}
