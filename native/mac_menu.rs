//! AppKit status item, transient popover and application menus. UI objects
//! live on the application thread and the popover's view is released on close.
use objc2::{
    AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class,
    msg_send,
    rc::{Retained, Weak},
    runtime::{AnyObject, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    NSBundle, NSData, NSDate, NSDateFormatter, NSDateFormatterStyle, NSNotification, NSNumber,
    NSObject, NSObjectProtocol, NSOperationQueue, NSPoint, NSRect, NSRectEdge, NSSize, NSString,
};
use serde_json::Value;
use std::{
    cell::{OnceCell, RefCell},
    ffi::c_void,
    rc::{Rc, Weak as StatusOwner},
};

pub type Callback = extern "C" fn(*mut c_void, i32);
const TOGGLE: i32 = 0;
pub const REFRESH: i32 = 1;
pub const DASHBOARD: i32 = 2;
pub const SETTINGS: i32 = 3;
pub const QUIT: i32 = 4;
pub const CLOSED: i32 = 5;
pub const POPOVER: i32 = 6;
const OPENED: i32 = 7;

struct Actions {
    context: *mut c_void,
    callback: Callback,
    monitor: OnceCell<StatusOwner<Status>>,
}
define_class!(
    #[unsafe(super=NSObject)]
    #[thread_kind=MainThreadOnly]
    #[ivars=Actions]
    #[name="QuotaBarRustMenuTarget"]
    struct Target;
    unsafe impl NSObjectProtocol for Target {}
    impl Target{
        #[unsafe(method(chosen:))]
        fn chosen(&self,sender:&AnyObject){
            let tag:isize=unsafe{msg_send![sender,tag]};
            self.dispatch(tag as i32);
        }
    }
    unsafe impl NSPopoverDelegate for Target{
        #[unsafe(method(popoverDidClose:))]
        fn closed(&self,_:&NSNotification){self.dispatch(CLOSED);}
    }
);
impl Target {
    fn dispatch(&self, tag: i32) {
        let action = self.ivars();
        let notify = |event| (action.callback)(action.context, event);
        let Some(owner) = action.monitor.get() else {
            notify(tag);
            return;
        };
        // Closing a popover can synchronously call its delegate. Keep the
        // controller alive if AppKit terminates or the service releases its handle.
        let Some(status) = owner.upgrade() else {
            return;
        };
        match tag {
            TOGGLE | POPOVER => {
                status.toggle();
                if status.shown() {
                    notify(OPENED);
                }
            }
            REFRESH => notify(REFRESH),
            DASHBOARD | SETTINGS => {
                status.close();
                notify(tag);
            }
            QUIT => NSApplication::sharedApplication(status.m).terminate(None),
            CLOSED => {
                status.clear();
                notify(CLOSED);
            }
            _ => {}
        }
    }

    fn new(m: MainThreadMarker, context: *mut c_void, callback: Callback) -> Retained<Self> {
        unsafe {
            msg_send![
                super(Self::alloc(m).set_ivars(Actions {
                    context,
                    callback,
                    monitor: OnceCell::new()
                })),
                init
            ]
        }
    }
}
fn s(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn image(data: &[u8], size: NSSize) -> Option<Retained<NSImage>> {
    let img = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(data))?;
    img.setSize(size);
    Some(img)
}

fn appearance(mode: &str) -> Option<Retained<NSAppearance>> {
    let name = match mode {
        "dark" => "NSAppearanceNameDarkAqua",
        "light" => "NSAppearanceNameAqua",
        _ => return None,
    };
    NSAppearance::appearanceNamed(&s(name))
}

pub fn apply_appearance(mode: &str) {
    if let Some(m) = MainThreadMarker::new() {
        NSApplication::sharedApplication(m).setAppearance(appearance(mode).as_deref());
    }
}

pub fn dock_visible(visible: bool) -> bool {
    let Some(m) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(m);
    let policy = if visible {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    if app.activationPolicy() == policy {
        return true;
    }
    let was_active = app.isActive();
    let key = app.keyWindow();
    if !app.setActivationPolicy(policy) {
        return false;
    }
    if was_active && let Some(key) = key {
        let window = Weak::from_retained(&key);
        let restore = block2::RcBlock::new(move || {
            if let Some(window) = window.load() {
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
                window.makeKeyAndOrderFront(None);
            }
        });
        // Focus may change while AppKit applies the activation policy. This block
        // only accesses the window on the main queue and does not keep it alive.
        unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&restore) };
    }
    true
}
pub struct Menus {
    target: Retained<Target>,
}
impl Menus {
    pub fn install(m: MainThreadMarker, context: *mut c_void, callback: Callback) -> Self {
        let target = Target::new(m, context, callback);
        let app = NSApplication::sharedApplication(m);
        let main = NSMenu::new(m);
        let menu = |title: &str| {
            let item = NSMenuItem::new(m);
            let sub = NSMenu::initWithTitle(NSMenu::alloc(m), &s(title));
            item.setSubmenu(Some(&sub));
            main.addItem(&item);
            sub
        };
        let add = |sub: &NSMenu, title: &str, selector: Sel, key: &str, tag: Option<i32>| unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(m),
                &s(title),
                Some(selector),
                &s(key),
            );
            if let Some(tag) = tag {
                item.setTag(tag as isize);
                item.setTarget(Some(&target));
            }
            sub.addItem(&item);
            item
        };
        let sep = |sub: &NSMenu| sub.addItem(&NSMenuItem::separatorItem(m));
        let top = menu("QuotaBar");
        add(
            &top,
            "QuotaBar 정보",
            sel!(orderFrontStandardAboutPanel:),
            "",
            None,
        );
        sep(&top);
        add(&top, "설정…", sel!(chosen:), ",", Some(SETTINGS));
        sep(&top);
        let services = NSMenu::initWithTitle(NSMenu::alloc(m), &s("서비스"));
        let item = NSMenuItem::new(m);
        item.setTitle(&s("서비스"));
        item.setSubmenu(Some(&services));
        top.addItem(&item);
        app.setServicesMenu(Some(&services));
        sep(&top);
        add(&top, "QuotaBar 가리기", sel!(hide:), "h", None);
        add(
            &top,
            "다른 항목 가리기",
            sel!(hideOtherApplications:),
            "h",
            None,
        )
        .setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Option);
        add(&top, "모두 보기", sel!(unhideAllApplications:), "", None);
        sep(&top);
        add(&top, "QuotaBar 종료", sel!(chosen:), "q", Some(QUIT));
        let edit = menu("Edit");
        add(&edit, "실행 취소", sel!(undo:), "z", None);
        add(&edit, "다시 실행", sel!(redo:), "Z", None);
        sep(&edit);
        for (label, sel, key) in [
            ("잘라내기", sel!(cut:), "x"),
            ("복사", sel!(copy:), "c"),
            ("붙여넣기", sel!(paste:), "v"),
            ("모두 선택", sel!(selectAll:), "a"),
        ] {
            add(&edit, label, sel, key, None);
        }
        let view = menu("View");
        add(&view, "전체 화면 전환", sel!(toggleFullScreen:), "f", None)
            .setKeyEquivalentModifierMask(
                NSEventModifierFlags::Command | NSEventModifierFlags::Control,
            );
        let monitor = menu("모니터링");
        for (label, key, tag) in [
            ("새로고침", "r", REFRESH),
            ("Dashboard 열기", "1", DASHBOARD),
            ("상단바 사용량 보기", "2", POPOVER),
        ] {
            add(&monitor, label, sel!(chosen:), key, Some(tag));
        }
        let window = menu("Window");
        for (label, selector, key) in [
            ("닫기", sel!(performClose:), "w"),
            ("최소화", sel!(performMiniaturize:), "m"),
            ("확대/축소", sel!(performZoom:), ""),
            ("모두 앞으로 가져오기", sel!(arrangeInFront:), ""),
        ] {
            add(&window, label, selector, key, None);
        }
        app.setWindowsMenu(Some(&window));
        app.setMainMenu(Some(&main));
        Self { target }
    }
}
fn title_image(mark: &Retained<NSImage>, font: &NSFont, title: &str) -> Retained<NSImage> {
    use objc2_foundation::{NSAttributedString, NSDictionary};
    let black = NSColor::blackColor();
    let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
    let attrs = NSDictionary::from_slices(&keys, &[font.as_ref() as &AnyObject, black.as_ref()]);
    let text = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &s(title),
            Some(&attrs),
        )
    };
    let mark = mark.clone();
    let logo = mark.size();
    let label = text.size();
    let x = logo.width + 6.;
    let size = NSSize::new(
        (x + label.width).ceil(),
        logo.height.max(label.height).ceil(),
    );
    let draw = block2::RcBlock::new(move |_: NSRect| {
        let frame = rect(
            0.,
            (size.height - logo.height) / 2.,
            logo.width,
            logo.height,
        );
        mark.drawInRect_fromRect_operation_fraction(
            frame,
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.,
        );
        text.drawAtPoint(NSPoint::new(x, (size.height - label.height) / 2.));
        objc2::runtime::Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(size, false, &draw);
    image.setTemplate(true);
    image
}
pub struct Status {
    m: MainThreadMarker,
    item: Retained<NSStatusItem>,
    menus: Menus,
    mark: Retained<NSImage>,
    title_image: RefCell<Option<(String, Retained<NSImage>)>>,
    state: RefCell<Value>,
    panel: RefCell<Option<Retained<NSPopover>>>,
}
impl Status {
    pub fn new(context: *mut c_void, callback: Callback) -> Option<Rc<Self>> {
        let m = MainThreadMarker::new()?;
        if let Some(icon) = NSBundle::mainBundle()
            .URLForResource_withExtension(Some(&s("AppDockIcon")), Some(&s("png")))
            .and_then(|url| NSImage::initWithContentsOfURL(NSImage::alloc(), &url))
        {
            icon.setSize(NSSize::new(256., 256.));
            unsafe { NSApplication::sharedApplication(m).setApplicationIconImage(Some(&icon)) };
        }
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.);
        let menus = Menus::install(m, context, callback);
        let button = item.button(m)?;
        button.setImageScaling(NSImageScaling::ScaleNone);
        unsafe {
            button.setTarget(Some(&menus.target));
            button.setAction(Some(sel!(chosen:)));
        }
        button.setTag(TOGGLE as isize);
        let mark = image(
            include_bytes!("../shared/assets/AppMark.svg"),
            NSSize::new(14. * 266. / 282., 14.),
        )?;
        mark.setTemplate(true);
        Some(Rc::new(Self {
            m,
            item,
            menus,
            mark,
            title_image: Default::default(),
            state: RefCell::new(Value::Null),
            panel: RefCell::default(),
        }))
    }
    pub fn monitor(context: *mut c_void, callback: Callback) -> Option<Rc<Self>> {
        let status = Self::new(context, callback)?;
        status
            .menus
            .target
            .ivars()
            .monitor
            .set(Rc::downgrade(&status))
            .ok()?;
        Some(status)
    }
    pub fn show_popover(&self) {
        self.menus.target.dispatch(POPOVER);
    }

    pub fn shown(&self) -> bool {
        self.panel.borrow().as_ref().is_some_and(|p| p.isShown())
    }
    pub fn clear(&self) {
        if let Some(p) = self.panel.borrow_mut().take() {
            p.setDelegate(None);
            p.setContentViewController(None);
        }
    }
    pub fn close(&self) {
        let panel = self.panel.borrow().clone();
        if let Some(p) = panel {
            p.close();
        }
    }
    pub fn update(&self, v: Value) {
        let mode = text(&v, "mode");
        if let Some(button) = self.item.button(self.m) {
            let font = NSFont::monospacedDigitSystemFontOfSize_weight(
                NSFont::menuBarFontOfSize(0.).pointSize(),
                unsafe { NSFontWeightRegular },
            );
            button.setFont(Some(&font));
            let title = match mode {
                "iconOnly" => "",
                "remainingOnly" => text(&v, "remaining"),
                _ => text(&v, "title"),
            };
            if mode == "remainingOnly" {
                button.setImage(None);
                button.setTitle(&s(title));
                button.setImagePosition(NSCellImagePosition::NoImage);
            } else {
                button.setTitle(&s(""));
                button.setImagePosition(NSCellImagePosition::ImageOnly);
                if mode == "iconOnly" {
                    button.setImage(Some(&self.mark));
                } else {
                    let mut cache = self.title_image.borrow_mut();
                    if cache.as_ref().is_none_or(|(old, _)| old != title) {
                        cache.replace((title.to_owned(), title_image(&self.mark, &font, title)));
                    }
                    button.setImage(cache.as_ref().map(|(_, img)| &**img));
                }
            }
            button.setToolTip(Some(&s(text(&v, "help"))));
            button.setAccessibilityLabel(Some(&s(&format!("남은 사용량 · {}", text(&v, "title")))));
        }
        self.state.replace(v);
        if self.shown() {
            self.rebuild();
        }
    }
    pub fn toggle(&self) {
        if self.shown() {
            self.close();
            return;
        }
        let Some(button) = self.item.button(self.m) else {
            return;
        };
        let panel = NSPopover::new(self.m);
        panel.setBehavior(NSPopoverBehavior::Transient);
        panel.setDelegate(Some(ProtocolObject::from_ref(&*self.menus.target)));
        self.panel.replace(Some(panel.clone()));
        self.rebuild();
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.m).activateIgnoringOtherApps(true);
        panel.showRelativeToRect_ofView_preferredEdge(button.bounds(), &button, NSRectEdge::MinY);
    }
    fn rebuild(&self) {
        let Some(panel) = self.panel.borrow().clone() else {
            return;
        };
        let v = self.state.borrow();
        let m = self.m;
        let dark = match text(&v, "appearance") {
            "dark" => Some(true),
            "light" => Some(false),
            _ => None,
        };
        panel.setAppearance(appearance(text(&v, "appearance")).as_deref());
        let accounts = v["accounts"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let measure = |width| {
            let headers: Vec<_> = accounts
                .iter()
                .map(|a| AccountHeader::new(a, m, width))
                .collect();
            let height = accounts
                .iter()
                .zip(&headers)
                .map(|(a, h)| account_height(a, h.height))
                .sum::<f64>()
                + 14. * accounts.len().saturating_sub(1) as f64;
            (headers, height)
        };
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(m), rect(0., 0., 294., 430.));
        scroll.setDrawsBackground(false);
        let (mut headers, mut list_height) = measure(294.);
        let overflow = list_height > 430.;
        let scroller_style = NSScroller::preferredScrollerStyle(m);
        scroll.setScrollerStyle(scroller_style);
        scroll.setHasVerticalScroller(overflow);
        // Before the popover is shown an empty clip view still reports its full
        // width. Ask AppKit for the occupied width with the user's scroller style.
        let list_width = unsafe {
            NSScrollView::contentSizeForFrameSize_horizontalScrollerClass_verticalScrollerClass_borderType_controlSize_scrollerStyle(
                NSSize::new(294., 430.), None, overflow.then(NSScroller::class),
                scroll.borderType(), NSControlSize::Regular, scroller_style, m,
            )
        }.width;
        if list_width != 294. {
            (headers, list_height) = measure(list_width);
        }
        let demo = v["demo"] == true;
        let pre = 18. + 24. + 14. + if demo { 27. } else { 0. };
        let list_top = pre + if accounts.is_empty() { 0. } else { 27. };
        let list_size = if accounts.is_empty() {
            15.
        } else {
            list_height.min(430.)
        };
        let height = list_top + list_size + 14. + 20. + 18.;
        let root = NSView::initWithFrame(NSView::alloc(m), rect(0., 0., 330., height));
        let view = View {
            native: &root,
            height,
            m,
        };
        let dark = dark.unwrap_or_else(|| {
            use objc2_foundation::NSArray;
            let aqua = s("NSAppearanceNameAqua");
            let dark_aqua = s("NSAppearanceNameDarkAqua");
            let names = NSArray::from_slice(&[aqua.as_ref(), dark_aqua.as_ref()]);
            NSApplication::sharedApplication(m)
                .effectiveAppearance()
                .bestMatchFromAppearancesWithNames(&names)
                .is_some_and(|name| name == dark_aqua)
        });
        let background = if dark { 0.075 } else { 1. };
        root.setWantsLayer(true);
        if let Some(layer) = root.layer() {
            layer.setBackgroundColor(Some(
                &NSColor::colorWithSRGBRed_green_blue_alpha(background, background, background, 1.)
                    .CGColor(),
            ));
        }
        let (tile, ink) = if dark {
            (NSColor::whiteColor(), NSColor::blackColor())
        } else {
            (NSColor::blackColor(), NSColor::whiteColor())
        };
        let logo = NSImageView::imageViewWithImage(&self.mark, m);
        let size = NSSize::new(24. * 266. / 456., 24. * 282. / 456.);
        logo.setFrame(view.rect(
            30. - size.width / 2.,
            30. - size.height / 2.,
            size.width,
            size.height,
        ));
        logo.setContentTintColor(Some(&ink));
        view.rounded(18., 18., 24., 24., 24. * 108. / 456., &tile);
        root.addSubview(&logo);
        view.label("QuotaBar", [50., 22., 180., 18.], 13., true, false);
        view.button(
            "새로고침",
            [286., 16., 29., 28.],
            REFRESH,
            &self.menus.target,
        );
        if demo {
            view.label(
                "미리보기 · 샘플 데이터",
                [18., 56., 292., 18.],
                11.,
                false,
                true,
            );
        }
        if accounts.is_empty() {
            view.label(text(&v, "empty"), [18., pre, 294., 16.], 12., false, true);
        } else {
            view.label("남은 사용량", [18., pre, 294., 16.], 11., true, false);
            let distance_from_top = panel.contentViewController().and_then(|ctl| {
                ctl.view().subviews().iter().find_map(|v| {
                    let scroll = v.downcast_ref::<NSScrollView>()?;
                    let document = scroll.documentView()?.bounds();
                    let visible = scroll.documentVisibleRect();
                    // AppKit measures this document from the bottom. Rows are laid
                    // out from the top and can grow during a live quota refresh.
                    Some((document.size.height - visible.origin.y - visible.size.height).max(0.))
                })
            });
            scroll.setFrame(view.rect(18., list_top, 294., list_size));
            let content =
                NSView::initWithFrame(NSView::alloc(m), rect(0., 0., list_width, list_height));
            let list = View {
                native: &content,
                height: list_height,
                m,
            };
            let mut y = 0.;
            for (a, header) in accounts.iter().zip(&headers) {
                list.account(a, header, y, dark);
                y += account_height(a, header.height) + 14.;
            }
            scroll.setDocumentView(Some(&content));
            let clip = scroll.contentView();
            let mut bounds = clip.bounds();
            bounds.origin.y = list_height - bounds.size.height - distance_from_top.unwrap_or(0.);
            clip.scrollToPoint(clip.constrainBoundsRect(bounds).origin);
            scroll.reflectScrolledClipView(&clip);
            root.addSubview(&scroll);
        }
        let footer = list_top + list_size + 14.;
        view.button(
            "Dashboard 열기",
            [18., footer, 89., 20.],
            DASHBOARD,
            &self.menus.target,
        );
        view.button(
            "설정…",
            [115., footer, 42., 20.],
            SETTINGS,
            &self.menus.target,
        );
        view.button("종료", [278., footer, 34., 20.], QUIT, &self.menus.target);
        let ctl = NSViewController::new(m);
        ctl.setView(&root);
        panel.setContentViewController(Some(&ctl));
        panel.setContentSize(NSSize::new(330., height));
    }
}
impl Drop for Status {
    fn drop(&mut self) {
        self.clear();
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
fn periods(a: &Value) -> &[Value] {
    a["windows"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
}
struct AccountHeader {
    label: Retained<NSTextField>,
    width: f64,
    height: f64,
}
impl AccountHeader {
    fn new(a: &Value, m: MainThreadMarker, list_width: f64) -> Self {
        let title = format!("{}({})", text(a, "provider"), text(a, "name"));
        let label = NSTextField::wrappingLabelWithString(&s(&title), m);
        let width = list_width - if a["loading"] == true { 56. } else { 32. };
        label.setFont(Some(&NSFont::systemFontOfSize_weight(12., unsafe {
            NSFontWeightMedium
        })));
        label.setPreferredMaxLayoutWidth(width);
        let height = label.cell().map_or(24., |cell| {
            cell.cellSizeForBounds(rect(0., 0., width, f64::MAX))
                .height
                .ceil()
                .max(24.)
        });
        Self {
            label,
            width,
            height,
        }
    }
}
fn account_height(a: &Value, header: f64) -> f64 {
    header
        + if periods(a).is_empty() {
            21.
        } else {
            periods(a)
                .iter()
                .map(|w| if w["resetAt"].is_null() { 32. } else { 50. })
                .sum()
        }
        + if a["error"] == true { 21. } else { 0. }
        + 15.
}
struct View<'a> {
    native: &'a NSView,
    height: f64,
    m: MainThreadMarker,
}
impl View<'_> {
    fn rect(&self, x: f64, y: f64, w: f64, h: f64) -> NSRect {
        rect(x, self.height - y - h, w, h)
    }
    fn label(
        &self,
        value: &str,
        [x, y, w, h]: [f64; 4],
        size: f64,
        bold: bool,
        muted: bool,
    ) -> Retained<NSTextField> {
        let t = NSTextField::labelWithString(&s(value), self.m);
        // NSTextFieldCell insets text two points from each horizontal edge.
        t.setFrame(self.rect(x - 2., y, w + 4., h));
        let font = if bold {
            NSFont::boldSystemFontOfSize(size)
        } else {
            NSFont::systemFontOfSize(size)
        };
        t.setFont(Some(&font));
        let color = if muted {
            NSColor::secondaryLabelColor()
        } else {
            NSColor::labelColor()
        };
        t.setTextColor(Some(&color));
        self.native.addSubview(&t);
        t
    }
    fn image(&self, img: &NSImage, x: f64, y: f64, w: f64, h: f64) {
        let view = NSImageView::imageViewWithImage(img, self.m);
        view.setFrame(self.rect(x, y, w, h));
        self.native.addSubview(&view);
    }
    fn button(&self, title: &str, [x, y, w, h]: [f64; 4], tag: i32, target: &Target) {
        let b = unsafe {
            NSButton::buttonWithTitle_target_action(
                &s(title),
                Some(target),
                Some(sel!(chosen:)),
                self.m,
            )
        };
        b.setTag(tag as isize);
        b.setFrame(self.rect(x, y, w, h));
        if tag == REFRESH {
            b.setBordered(false);
            b.setToolTip(Some(&s("새로고침")));
            if let Some(icon) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &s("arrow.clockwise"),
                Some(&s("새로고침")),
            ) {
                b.setImage(Some(&icon));
                b.setImagePosition(NSCellImagePosition::ImageOnly);
            }
        } else {
            b.setBordered(false);
            b.setFont(Some(&NSFont::systemFontOfSize(10.)));
            b.setWantsLayer(true);
            if let Some(layer) = b.layer() {
                layer.setCornerRadius(5.);
                layer.setBackgroundColor(Some(&NSColor::separatorColor().CGColor()));
            }
        }
        self.native.addSubview(&b);
    }
    fn bar(&self, x: f64, y: f64, w: f64, h: f64, color: &NSColor) -> Retained<NSView> {
        self.rounded(x, y, w, h, h / 2., color)
    }
    fn rounded(
        &self,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        radius: f64,
        color: &NSColor,
    ) -> Retained<NSView> {
        let view = NSView::initWithFrame(NSView::alloc(self.m), self.rect(x, y, w, h));
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setCornerRadius(radius);
            layer.setBackgroundColor(Some(&color.CGColor()));
        }
        self.native.addSubview(&view);
        view
    }
    fn account(&self, a: &Value, header: &AccountHeader, y: f64, dark: bool) {
        let width = self.native.bounds().size.width;
        let provider = text(a, "provider");
        let bytes: &[u8] = match provider {
            "Claude" => include_bytes!("../shared/assets/ClaudeLogo.svg"),
            "Gemini" => include_bytes!("../shared/assets/GeminiLogo.png"),
            _ => include_bytes!("../shared/assets/CodexLogo.png"),
        };
        let size = 24.
            * match provider {
                "Claude" => 0.75,
                "Gemini" => 0.82,
                _ => 1.,
            };
        if let Some(img) = image(bytes, NSSize::new(size, size)) {
            self.image(
                &img,
                (24. - size) / 2.,
                y + (header.height - size) / 2.,
                size,
                size,
            );
        }
        let label_height = header.label.cell().map_or(header.height, |cell| {
            cell.cellSizeForBounds(rect(0., 0., header.width, header.height))
                .height
        });
        header.label.setFrame(self.rect(
            30.,
            y + (header.height - label_height) / 2.,
            header.width + 4.,
            label_height,
        ));
        self.native.addSubview(&header.label);
        if a["loading"] == true {
            let progress = NSProgressIndicator::initWithFrame(
                NSProgressIndicator::alloc(self.m),
                self.rect(width - 16., y + (header.height - 16.) / 2., 16., 16.),
            );
            progress.setStyle(NSProgressIndicatorStyle::Spinning);
            progress.setControlSize(NSControlSize::Mini);
            unsafe {
                progress.startAnimation(None);
            }
            self.native.addSubview(&progress);
        }
        let mut top = y + header.height + 8.;
        if periods(a).is_empty() {
            self.label("데이터 대기 중", [0., top, 280., 18.], 11., false, true);
            top += 23.;
        }
        for w in periods(a) {
            let n = w["remaining"].as_f64().unwrap_or(0.).clamp(0., 100.);
            self.label(text(w, "title"), [0., top, 190., 16.], 11., false, true);
            let remaining = self.label(
                &format!("{}% 남음", n.round()),
                [width - 62., top, 62., 16.],
                11.,
                true,
                false,
            );
            remaining.setAlignment(NSTextAlignment::Right);
            remaining.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                11.,
                unsafe { NSFontWeightSemibold },
            )));
            if n <= 20. {
                remaining.setTextColor(Some(&NSColor::systemOrangeColor()));
            }
            let rgb = match (provider, dark) {
                ("Claude", false) => (0.659, 0.310, 0.149),
                ("Claude", true) => (0.902, 0.510, 0.290),
                ("Gemini", false) => (0.102, 0.451, 0.910),
                ("Gemini", true) => (0.541, 0.706, 0.973),
                (_, false) => (0.310, 0.361, 0.784),
                (_, true) => (0.537, 0.584, 1.),
            };
            let tint = if n <= 20. {
                NSColor::systemOrangeColor()
            } else {
                NSColor::colorWithSRGBRed_green_blue_alpha(rgb.0, rgb.1, rgb.2, 1.)
            };
            let track = self.bar(
                0.,
                top + 19.,
                width,
                5.,
                &NSColor::labelColor().colorWithAlphaComponent(0.06),
            );
            track.setAccessibilityElement(true);
            track.setAccessibilityLabel(Some(&s(&format!(
                "{} 잔여량 {} 퍼센트",
                text(w, "title"),
                n.trunc()
            ))));
            unsafe {
                track.setAccessibilityRole(Some(NSAccessibilityProgressIndicatorRole));
                track.setAccessibilityValue(Some(&NSNumber::new_f64(n)));
                track.setAccessibilityMinValue(Some(&NSNumber::new_f64(0.)));
                track.setAccessibilityMaxValue(Some(&NSNumber::new_f64(100.)));
            }
            if n > 0. {
                self.bar(
                    0.,
                    top + 19.,
                    width * n / 100.,
                    5.,
                    &tint.colorWithAlphaComponent(if a["stale"] == true { 0.45 } else { 1. }),
                );
            }
            if let Some(sec) = w["resetAt"].as_f64() {
                let date = NSDate::dateWithTimeIntervalSinceReferenceDate(sec);
                let reset = if date.timeIntervalSinceNow() <= 0. {
                    "리셋 시간 경과 · 새로고침 필요".into()
                } else {
                    format!(
                        "리셋 {}",
                        NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
                            &date,
                            NSDateFormatterStyle::MediumStyle,
                            NSDateFormatterStyle::ShortStyle
                        )
                    )
                };
                self.label(&reset, [0., top + 29., width, 14.], 9., false, true)
                    .setTextColor(Some(&NSColor::tertiaryLabelColor()));
                top += 18.;
            }
            top += 32.;
        }
        if a["error"] == true {
            self.label(
                "연결 확인 필요 · Dashboard에서 자세히 보기",
                [0., top, width, 18.],
                10.,
                false,
                true,
            )
            .setTextColor(Some(&NSColor::systemOrangeColor()));
        }
        self.bar(
            0.,
            y + account_height(a, header.height) - 1.,
            width,
            1.,
            &NSColor::separatorColor(),
        );
    }
}
