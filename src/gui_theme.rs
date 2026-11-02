//! Small native paint layer. Controls remain Windows controls with keyboard
//! access and UI Automation names; no web runtime or rasterized screen text.
use super::*;
pub const INK: COLORREF = COLORREF(0x00292019);
pub const MUTED: COLORREF = COLORREF(0x00746959);
pub const BLUE: COLORREF = COLORREF(0x00dd4949);
pub const SIDE: COLORREF = COLORREF(0x00fbf2e6);
pub const WHITE: COLORREF = COLORREF(0x00ffffff);
pub const LINE: COLORREF = COLORREF(0x00e3dcd5);
pub unsafe extern "system" fn button_proc(
    h: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEMOVE => {
                if GetPropW(h, w!("WechatRedReplyHot")).is_invalid() {
                    let _ = SetPropW(h, w!("WechatRedReplyHot"), Some(HANDLE(1usize as *mut _)));
                    let mut track = windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<
                            windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT,
                        >() as u32,
                        dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE,
                        hwndTrack: h,
                        dwHoverTime: 0,
                    };
                    let _ =
                        windows::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(&mut track);
                    let _ = InvalidateRect(Some(h), None, false);
                }
            }
            WM_MOUSELEAVE => {
                let _ = RemovePropW(h, w!("WechatRedReplyHot"));
                let _ = InvalidateRect(Some(h), None, false);
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(h, Some(button_proc), id);
            }
            _ => {}
        }
        DefSubclassProc(h, msg, w, l)
    }
}

pub struct Paint {
    pub white: HBRUSH,
    pub sidebar: HBRUSH,
}
impl Paint {
    pub unsafe fn new() -> Self {
        unsafe {
            Self {
                white: CreateSolidBrush(WHITE),
                sidebar: CreateSolidBrush(SIDE),
            }
        }
    }
}
impl Drop for Paint {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.white.into());
            let _ = DeleteObject(self.sidebar.into());
        }
    }
}
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}
pub unsafe fn rounded(dc: HDC, r: RECT, fill: COLORREF, border: COLORREF, radius: i32) {
    unsafe {
        let brush = CreateSolidBrush(fill);
        let pen = CreatePen(PS_SOLID, 1, border);
        let old_brush = SelectObject(dc, brush.into());
        let old_pen = SelectObject(dc, pen.into());
        let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
    }
}
unsafe fn text(dc: HDC, r: RECT, s: &str, font: HFONT, color: COLORREF, flags: DRAW_TEXT_FORMAT) {
    unsafe {
        let old = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, color);
        let mut r = r;
        let mut s = config::wide(s);
        s.pop();
        DrawTextW(dc, &mut s, &mut r, flags | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}
unsafe fn stroke(dc: HDC, points: &[(i32, i32)]) {
    unsafe {
        let points = points
            .iter()
            .map(|&(x, y)| POINT { x, y })
            .collect::<Vec<_>>();
        let _ = Polyline(dc, &points);
    }
}
pub unsafe fn icon(dc: HDC, id: i32, x: i32, y: i32, size: i32, color: COLORREF) {
    unsafe {
        let s = size as f64 / 24.;
        let p = |a: i32, b: i32| (x + (a as f64 * s) as i32, y + (b as f64 * s) as i32);
        let pen = CreatePen(PS_SOLID, (1.6 * s).ceil().max(1.) as i32, color);
        let old = SelectObject(dc, pen.into());
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let line =
            |a: &[(i32, i32)]| stroke(dc, &a.iter().map(|&(a, b)| p(a, b)).collect::<Vec<_>>());
        let ellipse = |a: i32, b: i32, c: i32, d: i32| {
            let (a, b) = p(a, b);
            let (c, d) = p(c, d);
            let _ = Ellipse(dc, a, b, c, d);
        };
        match id {
            101 | 640 => {
                ellipse(8, 2, 16, 10);
                line(&[(4, 22), (4, 18), (7, 14), (17, 14), (20, 18), (20, 22)]);
            }
            100 => {
                ellipse(5, 3, 12, 10);
                ellipse(14, 5, 20, 11);
                line(&[(2, 21), (2, 17), (5, 13), (12, 13), (15, 17), (15, 21)]);
                line(&[(16, 14), (20, 14), (23, 18), (23, 21)]);
            }
            104 => {
                line(&[(2, 11), (22, 2), (14, 22), (10, 14), (2, 11)]);
                line(&[(10, 14), (22, 2)]);
            }
            103 | 631 => {
                let (a, b) = p(2, 3);
                let (c, d) = p(22, 18);
                let _ = RoundRect(dc, a, b, c, d, 4, 4);
                line(&[(6, 18), (6, 23), (12, 18)]);
                for a in [7, 12, 17] {
                    ellipse(a, 9, a + 1, 10);
                }
            }
            102 => {
                let (a, b) = p(5, 5);
                let (c, d) = p(19, 19);
                let _ = Rectangle(dc, a, b, c, d);
                ellipse(9, 9, 15, 15);
                for a in [8, 15] {
                    line(&[(a, 2), (a, 5)]);
                    line(&[(a, 19), (a, 22)]);
                    line(&[(2, a), (5, a)]);
                    line(&[(19, a), (22, a)]);
                }
            }
            300 => line(&[(7, 3), (20, 12), (7, 21), (7, 3)]),
            301 => {
                let (a, b) = p(5, 5);
                let (c, d) = p(19, 19);
                let _ = Rectangle(dc, a, b, c, d);
            }
            302 => {
                line(&[(4, 3), (18, 3), (21, 6), (21, 21), (3, 21), (3, 3), (4, 3)]);
                line(&[(7, 3), (7, 10), (17, 10), (17, 3)]);
                line(&[(7, 21), (7, 15), (17, 15), (17, 21)]);
            }
            303 | 305 => {
                line(&[
                    (20, 8),
                    (17, 4),
                    (10, 3),
                    (5, 6),
                    (3, 12),
                    (5, 18),
                    (11, 21),
                    (18, 18),
                ]);
                line(&[(20, 2), (20, 8), (14, 8)]);
            }
            306 => {
                line(&[(8, 14), (5, 17), (5, 20), (8, 22), (11, 21), (15, 17)]);
                line(&[(9, 7), (13, 3), (17, 2), (21, 5), (21, 8), (17, 12)]);
                line(&[(8, 17), (17, 8)]);
            }
            304 | 317 => {
                let (a, b) = p(7, 7);
                let (c, d) = p(21, 22);
                let _ = RoundRect(dc, a, b, c, d, 3, 3);
                line(&[(4, 17), (2, 17), (2, 2), (17, 2), (17, 4)]);
            }
            307 => {
                line(&[
                    (1, 12),
                    (6, 6),
                    (12, 4),
                    (18, 6),
                    (23, 12),
                    (18, 18),
                    (12, 20),
                    (6, 18),
                    (1, 12),
                ]);
                ellipse(8, 8, 16, 16);
            }
            319 => line(&[(9, 5), (16, 12), (9, 19)]),
            _ => {}
        }
        SelectObject(dc, old_brush);
        SelectObject(dc, old);
        let _ = DeleteObject(pen.into());
    }
}
pub unsafe fn draw_button(app: &App, d: &DRAWITEMSTRUCT) {
    unsafe {
        let id = d.CtlID as i32;
        let dc = d.hDC;
        let r = d.rcItem;
        let s = app.scale;
        if id == 403 {
            let selected =
                d.itemState.0 & ODS_SELECTED.0 != 0 && d.itemState.0 & ODS_COMBOBOXEDIT.0 == 0;
            FillRect(
                dc,
                &r,
                if selected {
                    app.paint.sidebar
                } else {
                    app.paint.white
                },
            );
            let label = if d.itemID == u32::MAX {
                "请选择微信窗口".to_owned()
            } else {
                let length = SendMessageW(
                    d.hwndItem,
                    CB_GETLBTEXTLEN,
                    Some(WPARAM(d.itemID as usize)),
                    None,
                )
                .0;
                if !(0..32768).contains(&length) {
                    return;
                }
                let mut buffer = vec![0u16; length as usize + 1];
                let n = SendMessageW(
                    d.hwndItem,
                    CB_GETLBTEXT,
                    Some(WPARAM(d.itemID as usize)),
                    Some(LPARAM(buffer.as_mut_ptr() as isize)),
                )
                .0;
                if n < 0 {
                    return;
                }
                String::from_utf16_lossy(&buffer[..n as usize])
            };
            let mut content = r;
            content.left += (7. * s) as i32;
            content.right -= (5. * s) as i32;
            text(
                dc,
                content,
                &label,
                app.font,
                INK,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
            if d.itemState.0 & ODS_FOCUS.0 != 0 {
                let _ = DrawFocusRect(dc, &r);
            }
            return;
        }
        let nav = (100..=104).contains(&id);
        let selected = nav && app.page == id;
        let disabled = d.itemState.0 & ODS_DISABLED.0 != 0;
        let pressed = d.itemState.0 & ODS_SELECTED.0 != 0;
        let hot = !GetPropW(d.hwndItem, w!("WechatRedReplyHot")).is_invalid();
        let active = selected || ([311, 312, 313, 314, 318].contains(&id) && app.log_filter == id);
        let primary = matches!(id, 302 | 306);
        let fill = if disabled {
            COLORREF(0x00eeeae5)
        } else if primary {
            if pressed { COLORREF(0x00bb3335) } else { BLUE }
        } else if active {
            COLORREF(0x00f9e4d2)
        } else if pressed {
            COLORREF(0x00f4eee7)
        } else if hot {
            COLORREF(0x00f9efe5)
        } else if nav {
            SIDE
        } else {
            WHITE
        };
        let color = if disabled {
            COLORREF(0x008d8274)
        } else if primary {
            WHITE
        } else if active {
            COLORREF(0x009c392e)
        } else {
            INK
        };
        let border = if nav || active {
            fill
        } else if primary {
            BLUE
        } else {
            LINE
        };
        let brush = if nav {
            app.paint.sidebar
        } else {
            app.paint.white
        };
        FillRect(dc, &r, brush);
        rounded(
            dc,
            rect(
                r.left + 1,
                r.top + 1,
                r.right - r.left - 2,
                r.bottom - r.top - 2,
            ),
            fill,
            border,
            (10. * s) as i32,
        );
        let has_icon = nav || matches!(id, 300..=307 | 317 | 319);
        let label = app.get(id);
        let mut tr = r;
        if id == 307 {
            icon(
                dc,
                id,
                (r.left + r.right - (20. * s) as i32) / 2,
                (r.top + r.bottom - (20. * s) as i32) / 2,
                (20. * s) as i32,
                color,
            );
            if d.itemState.0 & ODS_FOCUS.0 != 0 {
                let _ = DrawFocusRect(
                    dc,
                    &rect(
                        r.left + 4,
                        r.top + 4,
                        r.right - r.left - 8,
                        r.bottom - r.top - 8,
                    ),
                );
            }
            return;
        }
        if nav || id == 319 {
            icon(
                dc,
                id,
                r.left + (16. * s) as i32,
                r.top + ((r.bottom - r.top) - (21. * s) as i32) / 2,
                (21. * s) as i32,
                color,
            );
            tr.left += (49. * s) as i32;
        } else if has_icon {
            let old = SelectObject(dc, app.font.into());
            let mut bounds = RECT::default();
            let mut wide = config::wide(&label);
            wide.pop();
            DrawTextW(dc, &mut wide, &mut bounds, DT_CALCRECT | DT_SINGLELINE);
            SelectObject(dc, old);
            let total = bounds.right + (29. * s) as i32;
            let start = (r.right + r.left - total) / 2;
            icon(
                dc,
                id,
                start,
                r.top + ((r.bottom - r.top) - (18. * s) as i32) / 2,
                (18. * s) as i32,
                color,
            );
            tr.left = start + (29. * s) as i32;
            tr.right = r.right - (5. * s) as i32;
        }
        text(
            dc,
            tr,
            &label,
            app.font,
            color,
            DT_SINGLELINE | DT_VCENTER | if nav || has_icon { DT_LEFT } else { DT_CENTER },
        );
        if d.itemState.0 & ODS_FOCUS.0 != 0 {
            let focus = rect(
                r.left + 4,
                r.top + 4,
                r.right - r.left - 8,
                r.bottom - r.top - 8,
            );
            let _ = DrawFocusRect(dc, &focus);
        }
    }
}
pub unsafe fn paint(app: &App, dc: HDC) {
    unsafe {
        let mut client = RECT::default();
        let _ = GetClientRect(app.root, &mut client);
        FillRect(dc, &client, app.paint.white);
        let s = app.scale;
        let side = app.side_width();
        FillRect(
            dc,
            &rect(0, 0, (side as f64 * s) as i32, client.bottom),
            app.paint.sidebar,
        );
        icon(
            dc,
            631,
            (22. * s) as i32,
            (22. * s) as i32,
            (23. * s) as i32,
            INK,
        );
        let x = ((side + 28) as f64 * s) as i32;
        if app.page == 101 {
            let card = app.account_rect();
            let r = rect(
                (card.left as f64 * s) as i32,
                (card.top as f64 * s) as i32,
                ((card.right - card.left) as f64 * s) as i32,
                ((card.bottom - card.top) as f64 * s) as i32,
            );
            rounded(dc, r, WHITE, LINE, (14. * s) as i32);
            let av = 56. * app.vertical();
            let iy = r.top + ((r.bottom - r.top) - (av * s) as i32) / 2;
            rounded(
                dc,
                rect(
                    r.left + (20. * s) as i32,
                    iy,
                    (av * s) as i32,
                    (av * s) as i32,
                ),
                COLORREF(0x00fcf4ec),
                COLORREF(0x00fcf4ec),
                (12. * s) as i32,
            );
            icon(
                dc,
                640,
                r.left + ((20. + av * 0.2) * s) as i32,
                iy + (av * 0.2 * s) as i32,
                (av * 0.6 * s) as i32,
                INK,
            );
        }
        for &id in &[
            200, 201, 404, 500, 501, 502, 503, 504, 505, 506, 507, 508, 509, 510,
        ] {
            if let Some(&control) = app.controls.get(&id) {
                if !IsWindowVisible(control).as_bool() {
                    continue;
                }
                let mut r = RECT::default();
                if GetWindowRect(control, &mut r).is_err() {
                    continue;
                }
                let mut p = POINT {
                    x: r.left,
                    y: r.top,
                };
                let _ = ScreenToClient(app.root, &mut p);
                let pad = (8. * s) as i32;
                let py = (4. * s) as i32;
                let frame = rect(
                    p.x - pad,
                    p.y - py,
                    r.right - r.left + pad * 2,
                    r.bottom - r.top + py * 2,
                );
                rounded(dc, frame, WHITE, LINE, (8. * s) as i32);
            }
        }
        let y = client.bottom - (132. * s) as i32;
        let pen = CreatePen(PS_SOLID, 1, LINE);
        let old = SelectObject(dc, pen.into());
        stroke(dc, &[(x, y), (client.right - (28. * s) as i32, y)]);
        SelectObject(dc, old);
        let _ = DeleteObject(pen.into());
    }
}
