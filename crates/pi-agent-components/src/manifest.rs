use serde::Serialize;

/// A pinned third-party FFmpeg release. URLs are intentionally not floating.
#[derive(Debug, Clone, Serialize)]
pub struct FfmpegRelease {
    pub version: &'static str,
    pub architecture: &'static str,
    pub asset: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FfmpegManifest;

impl FfmpegManifest {
    pub const RELEASE_TAG: &'static str = "autobuild-2026-08-04-21-26";
    pub const LICENSE: &'static str = "LGPL";
    pub const BUILD_PROJECT_URL: &'static str = "https://github.com/BtbN/FFmpeg-Builds";
    pub const SOURCE_URL: &'static str = "https://github.com/FFmpeg/FFmpeg/tree/9b6c8969e0";

    pub fn x64() -> FfmpegRelease {
        Self::release(
            "x64",
            "ffmpeg-n8.1.2-34-g9b6c8969e0-win64-lgpl-8.1.zip",
            "534b3bfe48de5d3d181430294602a5977a198a47a28e48a1599545eab0ab7a60",
        )
    }

    pub fn arm64() -> FfmpegRelease {
        Self::release(
            "arm64",
            "ffmpeg-n8.1.2-34-g9b6c8969e0-winarm64-lgpl-8.1.zip",
            "69ae299f3e8a0795e4fd3def0678e7ac71dbfec6795199d425b04608c71179c4",
        )
    }

    fn release(
        architecture: &'static str,
        asset: &'static str,
        sha256: &'static str,
    ) -> FfmpegRelease {
        FfmpegRelease {
            version: "n8.1.2-34-g9b6c8969e0-lgpl-8.1",
            architecture,
            asset,
            url: match architecture {
                "x64" => "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-04-21-26/ffmpeg-n8.1.2-34-g9b6c8969e0-win64-lgpl-8.1.zip",
                "arm64" => "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-04-21-26/ffmpeg-n8.1.2-34-g9b6c8969e0-winarm64-lgpl-8.1.zip",
                _ => unreachable!("manifest architecture is fixed"),
            },
            sha256,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_releases_have_expected_hashes() {
        assert_eq!(
            FfmpegManifest::x64().sha256,
            "534b3bfe48de5d3d181430294602a5977a198a47a28e48a1599545eab0ab7a60"
        );
        assert_eq!(
            FfmpegManifest::arm64().sha256,
            "69ae299f3e8a0795e4fd3def0678e7ac71dbfec6795199d425b04608c71179c4"
        );
        assert!(!FfmpegManifest::x64().url.contains("latest"));
    }
}
