pub const APP_ID: &str = "io.github.arthjean.CrowBot";

pub const APP_NAME: &str = "Crow Bot";

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg;

    use super::*;

    const PNG_SIZES: [u32; 4] = [48, 128, 256, 512];

    fn manifest_dir() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    fn repo() -> PathBuf {
        manifest_dir().join("../..")
    }

    fn resources() -> PathBuf {
        manifest_dir().join("resources/linux")
    }

    fn desktop_script() -> PathBuf {
        repo().join("scripts/desktop-files.sh")
    }

    fn files(folder: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![folder.to_path_buf()];
        while let Some(folder) = pending.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    found.push(path);
                }
            }
        }
        found.sort();
        found
    }

    fn relative(path: &Path) -> String {
        path.strip_prefix(resources())
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn scalable_icon() -> PathBuf {
        resources().join(format!("icons/hicolor/scalable/apps/{APP_ID}.svg"))
    }

    fn png_icon(size: u32) -> PathBuf {
        resources().join(format!("icons/hicolor/{size}x{size}/apps/{APP_ID}.png"))
    }

    fn desktop_entry() -> PathBuf {
        resources().join(format!("applications/{APP_ID}.desktop"))
    }

    fn render(size: u32) -> Pixmap {
        let data = std::fs::read(scalable_icon()).unwrap();
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).unwrap();
        let mut pixmap = Pixmap::new(size, size).unwrap();
        let scale = size as f32 / tree.size().width();
        resvg::render(
            &tree,
            Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        pixmap
    }

    fn application_ids(text: &str) -> Vec<&str> {
        const PREFIXES: [&str; 6] = ["io.", "com.", "org.", "net.", "dev.", "app."];
        let is_part = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        let mut found = Vec::new();
        for (start, _) in text.char_indices() {
            let before = text[..start].chars().next_back();
            if before.is_some_and(is_part) || !PREFIXES.iter().any(|p| text[start..].starts_with(p))
            {
                continue;
            }
            let end = text[start..]
                .find(|c: char| !is_part(c))
                .map_or(text.len(), |end| start + end);
            let name = text[start..end].trim_end_matches('.');
            let name = [".desktop", ".svg", ".png"]
                .iter()
                .find_map(|extension| name.strip_suffix(extension))
                .unwrap_or(name);
            if name.split('.').filter(|part| !part.is_empty()).count() >= 3 {
                found.push(name);
            }
        }
        found
    }

    fn foreign_ids(paths: &[PathBuf]) -> Vec<String> {
        let mut foreign = Vec::new();
        for path in paths {
            let file_name = path.file_name().unwrap().to_string_lossy();
            let mut names: Vec<String> = application_ids(&file_name)
                .into_iter()
                .map(str::to_owned)
                .collect();
            if let Ok(text) = std::fs::read_to_string(path) {
                names.extend(application_ids(&text).into_iter().map(str::to_owned));
            }
            for name in names {
                if name != APP_ID {
                    foreign.push(format!("{} names {name}", path.display()));
                }
            }
        }
        foreign
    }

    #[test]
    fn the_application_id_is_written_once_in_the_client_source() {
        let sources: Vec<PathBuf> = files(&manifest_dir().join("src"))
            .into_iter()
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .collect();
        let holders: Vec<String> = sources
            .iter()
            .flat_map(|path| {
                let text = std::fs::read_to_string(path).unwrap();
                text.lines()
                    .filter(|line| line.contains(APP_ID))
                    .map(|line| format!("{}: {}", path.display(), line.trim()))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(holders.len(), 1, "{holders:#?}");
        assert!(
            holders[0].contains("identity.rs: pub const APP_ID: &str ="),
            "{holders:#?}"
        );
    }

    #[test]
    fn every_linux_resource_is_named_after_the_application_id() {
        let mut expected = vec![relative(&desktop_entry()), relative(&scalable_icon())];
        expected.extend(PNG_SIZES.map(|size| relative(&png_icon(size))));
        expected.sort();
        let found: Vec<String> = files(&resources())
            .iter()
            .map(|path| relative(path))
            .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn no_linux_resource_names_another_application_id() {
        let mut paths = files(&resources());
        paths.push(desktop_script());
        let foreign = foreign_ids(&paths);
        assert!(foreign.is_empty(), "{foreign:#?}");
    }

    #[test]
    fn a_file_that_names_another_application_id_is_named() {
        let folder = scratch("foreign-id");
        let renamed = folder.join("io.github.someone.Other.desktop");
        std::fs::write(&renamed, "[Desktop Entry]\n").unwrap();
        let inside = folder.join("entry.desktop");
        std::fs::write(
            &inside,
            format!("Icon={APP_ID}\nStartupWMClass=org.example.Crow\n"),
        )
        .unwrap();
        let own = folder.join(format!("{APP_ID}.png"));
        std::fs::write(&own, [0x89, b'P', b'N', b'G']).unwrap();
        let foreign = foreign_ids(&[renamed.clone(), inside.clone(), own]);
        assert_eq!(
            foreign,
            [
                format!("{} names io.github.someone.Other", renamed.display()),
                format!("{} names org.example.Crow", inside.display()),
            ]
        );
    }

    #[test]
    fn the_desktop_entry_names_the_application_and_its_icon() {
        let text = std::fs::read_to_string(desktop_entry()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.first(), Some(&"[Desktop Entry]"));
        for line in [
            "Type=Application".to_owned(),
            format!("Name={APP_NAME}"),
            format!("Icon={APP_ID}"),
            "Exec=crowbot".to_owned(),
            "Terminal=false".to_owned(),
        ] {
            assert!(lines.contains(&line.as_str()), "{line} is missing");
        }
        assert!(!text.contains("StartupWMClass"));
    }

    #[test]
    fn the_scalable_icon_is_the_mascot_at_rest_with_a_crest() {
        let icon = std::fs::read_to_string(scalable_icon()).unwrap();
        for layer in ["head_crest", "beak", "eyes", "pupils_rest"] {
            let text =
                std::fs::read_to_string(manifest_dir().join(format!("assets/mascot/{layer}.svg")))
                    .unwrap();
            let start = text.find('>').unwrap() + 1;
            let end = text.rfind("</svg>").unwrap();
            assert!(
                icon.contains(&text[start..end]),
                "{layer} is not in the icon"
            );
        }
    }

    #[test]
    fn each_png_icon_is_the_scalable_icon_at_its_size() {
        for size in PNG_SIZES {
            let stored = Pixmap::load_png(png_icon(size)).unwrap();
            assert_eq!((stored.width(), stored.height()), (size, size));
            let rendered = render(size);
            let worst = stored
                .data()
                .iter()
                .zip(rendered.data())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                worst <= 2,
                "{size} px differs by {worst}: run `cargo test write_the_png_icons -- --ignored`"
            );
        }
    }

    #[test]
    #[ignore = "writes the PNG icons from the scalable icon"]
    fn write_the_png_icons() {
        for size in PNG_SIZES {
            let path = png_icon(size);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            render(size).save_png(path).unwrap();
        }
    }

    fn luminance([r, g, b]: [u8; 3]) -> f64 {
        let linear = |channel: u8| {
            let c = f64::from(channel) / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }

    fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn the_outline_of_the_48_px_icon_contrasts_with_light_and_dark_backgrounds() {
        let backgrounds = [
            ("white", [0xff, 0xff, 0xff]),
            ("Adwaita light", [0xfa, 0xfa, 0xfb]),
            ("Adwaita dark", [0x22, 0x22, 0x26]),
            ("black", [0x00, 0x00, 0x00]),
        ];
        let icon = Pixmap::load_png(png_icon(48)).unwrap();
        let size = 48_i32;
        let alpha = |x: i32, y: i32| {
            if (0..size).contains(&x) && (0..size).contains(&y) {
                icon.pixel(x as u32, y as u32).unwrap().alpha()
            } else {
                0
            }
        };
        let mut outline = Vec::new();
        for y in 0..size {
            for x in 0..size {
                let edge = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                    .iter()
                    .any(|(dx, dy)| alpha(x + dx, y + dy) < 255);
                if alpha(x, y) == 255 && edge {
                    let pixel = icon.pixel(x as u32, y as u32).unwrap();
                    outline.push([pixel.red(), pixel.green(), pixel.blue()]);
                }
            }
        }
        assert!(outline.len() > 48, "{} outline pixels", outline.len());
        for (name, background) in backgrounds {
            let worst = outline
                .iter()
                .map(|color| contrast(*color, background))
                .fold(f64::INFINITY, f64::min);
            assert!(worst >= 3.0, "{worst:.2} to 1 on {name}");
        }
    }

    /// A folder of the test, removed with it, also when the test fails.
    fn scratch(name: &str) -> crowbot_protocol::scratch::Scratch {
        crowbot_protocol::scratch::Scratch::new(&format!("crowbot-identity-{name}"))
    }

    #[cfg(target_os = "linux")]
    fn run_script(home: &Path, args: &[&str]) -> std::process::Output {
        let output = std::process::Command::new("sh")
            .arg(desktop_script())
            .args(args)
            .env("HOME", home.join("home"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_desktop_files_install_on_the_built_client_and_remove_to_nothing() {
        use std::os::unix::fs::PermissionsExt;

        let home = scratch("install");
        let bin = home.join("build $1 \"x\"");
        std::fs::create_dir_all(&bin).unwrap();
        let client = bin.join("crowbot");
        std::fs::write(&client, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(bin.join("crowbotd"), "").unwrap();

        run_script(&home, &["install", "--client", client.to_str().unwrap()]);

        let data = home.join("data");
        let installed: Vec<String> = files(&data)
            .iter()
            .map(|path| {
                path.strip_prefix(&data)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let expected: Vec<String> = files(&resources())
            .iter()
            .map(|path| relative(path))
            .collect();
        assert_eq!(installed, expected);
        for path in files(&resources()) {
            if path
                .extension()
                .is_some_and(|extension| extension != "desktop")
            {
                assert_eq!(
                    std::fs::read(data.join(relative(&path))).unwrap(),
                    std::fs::read(&path).unwrap()
                );
            }
        }
        let entry = data.join(format!("applications/{APP_ID}.desktop"));
        let text = std::fs::read_to_string(&entry).unwrap();
        let exec = text.lines().find(|line| line.starts_with("Exec=")).unwrap();
        let expected_exec = format!(
            "Exec=\"{}\"",
            client
                .to_str()
                .unwrap()
                .replace('$', "\\\\$")
                .replace('"', "\\\\\"")
        );
        assert_eq!(exec, expected_exec);
        assert!(text.contains(&format!("TryExec={}\n", client.display())));
        assert!(text.contains(&format!("Icon={APP_ID}\n")));
        if let Ok(validated) = std::process::Command::new("desktop-file-validate")
            .arg(&entry)
            .output()
        {
            assert!(
                validated.status.success() && validated.stdout.is_empty(),
                "{}",
                String::from_utf8_lossy(&validated.stdout)
            );
        }

        run_script(&home, &["remove"]);

        assert_eq!(files(&data), Vec::<PathBuf>::new());
        assert!(!home.join("state/crowbot/desktop-files").exists());
    }
}
