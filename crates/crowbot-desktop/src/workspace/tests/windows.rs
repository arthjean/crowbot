//! The title band of Windows, as it was before the Linux port: the three
//! caption buttons at the right end of every screen's band, and no window
//! button or frame of the Linux chrome.

use super::*;

/// The caption buttons sit at the right end of the band, in the order
/// minimize, maximize, close, each as wide as `CAPTION_BUTTON` and as tall as
/// the band, with no frame margin around the window.
fn assert_caption_buttons(cx: &mut VisualTestContext, screen: &str) {
    let width = cx.update(|window, _| window.viewport_size().width);
    let minimize = bounds(cx, "window-minimize");
    let maximize = bounds(cx, "window-maximize");
    let close = bounds(cx, "window-close");
    for button in [minimize, maximize, close] {
        assert_eq!(button.size.width, layout::CAPTION_BUTTON, "{screen}");
        assert_eq!(button.size.height, layout::TOP_ROW, "{screen}");
        assert_eq!(button.top(), px(0.), "{screen}");
    }
    assert_eq!(minimize.right(), maximize.left(), "{screen}");
    assert_eq!(maximize.right(), close.left(), "{screen}");
    assert_eq!(close.right(), width, "{screen}");
    for linux in ["window-buttons-left", "window-buttons-right"] {
        assert!(!shown(cx, linux), "{screen}: {linux}");
    }
}

#[gpui::test]
fn every_screen_keeps_the_caption_buttons_of_windows(cx: &mut TestAppContext) {
    let (workspace, mut daemon, cx) = open(cx);
    assert_caption_buttons(cx, "loading");

    daemon.connect(vec![summary(1, "Ada", BotState::Idle)], cx);
    assert_caption_buttons(cx, "workspace");
    // The workspace fills the window: no shadow margin, and the frame fill
    // is the workspace's own, as before.
    assert_eq!(bounds(cx, "rail-logo").origin, point(px(0.), px(0.)));
    assert!(workspace.read_with(cx, |workspace, _| !workspace.frame.client));

    click(cx, "open-settings");
    assert!(shown(cx, "settings-page"));
    assert_caption_buttons(cx, "settings");
    daemon.sent();
}
