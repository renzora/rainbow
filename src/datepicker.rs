//! A date field with a calendar pop-up, drawn in Rainbow's style: the date shown the way the
//! user's language and region write dates, a month grid with the week starting where it does
//! locally, and the years limited to the ones the catalogue has charts from.

use eframe::egui;
use egui::{Color32, CornerRadius, RichText, Sense, vec2};
use jiff::civil::Date;

use crate::i18n::t;
use crate::{n, theme};

const MONTHS: [&str; 12] = [
    n("January"),
    n("February"),
    n("March"),
    n("April"),
    n("May"),
    n("June"),
    n("July"),
    n("August"),
    n("September"),
    n("October"),
    n("November"),
    n("December"),
];
/// Monday first.
const WEEKDAYS: [&str; 7] = [n("Mo"), n("Tu"), n("We"), n("Th"), n("Fr"), n("Sa"), n("Su")];

/// How dates are written: the order of day, month and year, and the separator.
#[derive(Clone, Copy, PartialEq)]
enum Order {
    Dmy(char),
    Mdy(char),
    Ymd(char),
}

fn order() -> Order {
    let lang = crate::i18n::current();
    let region = sys_locale::get_locale().unwrap_or_default();
    match lang.split('-').next().unwrap_or("en") {
        // The US (and the Philippines) write month/day/year; other English speakers day/month/year.
        "en" if region.ends_with("US") || region.ends_with("PH") => Order::Mdy('/'),
        "ja" | "zh" | "ko" => Order::Ymd('/'),
        "sv" => Order::Ymd('-'),
        "de" | "ru" | "uk" | "cs" | "pl" | "tr" => Order::Dmy('.'),
        "nl" => Order::Dmy('-'),
        _ => Order::Dmy('/'),
    }
}

/// The week starts on Sunday in the US, Monday elsewhere.
fn sunday_first() -> bool {
    matches!(order(), Order::Mdy(_))
}

/// A date as the user's language writes it.
pub fn format(d: Date) -> String {
    let (y, m, day) = (d.year(), d.month(), d.day());
    match order() {
        Order::Dmy(s) => format!("{day:02}{s}{m:02}{s}{y}"),
        Order::Mdy(s) => format!("{m:02}{s}{day:02}{s}{y}"),
        Order::Ymd(s) => format!("{y}{s}{m:02}{s}{day:02}"),
    }
}

/// A date field: shows `value` (YYYY-MM-DD, or empty for none) and opens a calendar to pick one.
/// `years` is the range the year picker offers. Returns whether the date changed.
pub fn date_field(ui: &mut egui::Ui, id: &str, value: &mut String, years: std::ops::RangeInclusive<i16>, width: f32) -> bool {
    let date = value.trim().parse::<Date>().ok();
    let shown = date.map_or_else(|| t("Any date"), format);
    let text = if date.is_some() { RichText::new(shown) } else { RichText::new(shown).weak() };
    let button = egui::Button::new(text).min_size(vec2(width, 24.0));
    let resp = ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand);
    // The calendar icon at the button's far end.
    let icon_at = if crate::i18n::rtl() { resp.rect.left_center() + vec2(14.0, 0.0) } else { resp.rect.right_center() - vec2(14.0, 0.0) };
    ui.painter().text(icon_at, egui::Align2::CENTER_CENTER, "📅", egui::FontId::proportional(14.0), ui.visuals().weak_text_color());

    // The month on show in the calendar, kept while it's open.
    let view_id = egui::Id::new(("date_view", id));
    let mut changed = false;
    egui::Popup::from_toggle_button_response(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let today = jiff::Zoned::now().date();
            let mut view: Date = ui.data(|d| d.get_temp(view_id)).unwrap_or_else(|| date.unwrap_or(today).first_of_month());
            let year_id = view_id.with("years");
            let mut picking_year = ui.data(|d| d.get_temp::<bool>(year_id)).unwrap_or(false);
            let month_id = view_id.with("months");
            let mut picking_month = ui.data(|d| d.get_temp::<bool>(month_id)).unwrap_or(false);
            ui.set_width(7.0 * 34.0);

            // Month, with arrows either side and the year to pick from.
            crate::theme::hrow(ui, |ui| {
                if ui.add(egui::Button::new("‹").min_size(vec2(28.0, 26.0))).on_hover_text(t("Previous month")).clicked() {
                    view = view.checked_sub(jiff::Span::new().months(1)).unwrap_or(view);
                }
                // The month: click it to pick from the twelve (in place of the days, like the year).
                let month_label = format!("{}  ⏷", t(MONTHS[view.month() as usize - 1]));
                if ui.add(egui::Button::new(RichText::new(month_label).strong().size(15.0)).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    picking_month = !picking_month;
                    picking_year = false;
                }
                // The year: click it to pick from the years (shown in place of the days, so the
                // calendar doesn't close as it would for a separate drop-down).
                let year_label = format!("{}  ⏷", view.year());
                if ui.add(egui::Button::new(RichText::new(year_label).strong().size(15.0)).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    picking_year = !picking_year;
                    picking_month = false;
                }
                ui.with_layout(crate::theme::trailing(), |ui| {
                    if ui.add(egui::Button::new("›").min_size(vec2(28.0, 26.0))).on_hover_text(t("Next month")).clicked() {
                        view = view.checked_add(jiff::Span::new().months(1)).unwrap_or(view);
                    }
                });
            });
            ui.add_space(4.0);

            if picking_month {
                // The months, three to a row.
                for row in 0..4u8 {
                    crate::theme::hrow(ui, |ui| {
                        for col in 0..3u8 {
                            let m = row * 3 + col + 1;
                            let on = m as i8 == view.month();
                            let (r, cell) = ui.allocate_exact_size(vec2(72.0, 30.0), Sense::click());
                            let fill = if on {
                                theme::accent()
                            } else if cell.hovered() {
                                ui.visuals().widgets.hovered.bg_fill
                            } else {
                                Color32::TRANSPARENT
                            };
                            ui.painter().rect_filled(r, CornerRadius::same(6), fill);
                            let text = if on { theme::on_accent() } else { ui.visuals().text_color() };
                            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, t(MONTHS[m as usize - 1]), egui::FontId::proportional(13.0), text);
                            if cell.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                view = Date::new(view.year(), m as i8, 1).unwrap_or(view);
                                picking_month = false;
                            }
                        }
                    });
                }
                ui.data_mut(|d| {
                    d.insert_temp(view_id, view);
                    d.insert_temp(month_id, picking_month);
                });
                return;
            }
            if picking_year {
                // The years, newest first, four to a row.
                let all: Vec<i16> = years.clone().rev().collect();
                egui::ScrollArea::vertical().max_height(6.0 * 32.0).show(ui, |ui| {
                    for row in all.chunks(4) {
                        crate::theme::hrow(ui, |ui| {
                            for &y in row {
                                let on = y == view.year();
                                let (r, cell) = ui.allocate_exact_size(vec2(52.0, 28.0), Sense::click());
                                let fill = if on {
                                    theme::accent()
                                } else if cell.hovered() {
                                    ui.visuals().widgets.hovered.bg_fill
                                } else {
                                    Color32::TRANSPARENT
                                };
                                ui.painter().rect_filled(r, CornerRadius::same(6), fill);
                                let text = if on { theme::on_accent() } else { ui.visuals().text_color() };
                                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, y.to_string(), egui::FontId::proportional(13.0), text);
                                if cell.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    view = Date::new(y, view.month(), 1).unwrap_or(view);
                                    picking_year = false;
                                }
                            }
                        });
                    }
                });
                ui.data_mut(|d| {
                    d.insert_temp(view_id, view);
                    d.insert_temp(year_id, picking_year);
                    d.insert_temp(month_id, false);
                });
                return;
            }

            // Days of the week, then the month's days in a grid.
            let sunday = sunday_first();
            let weekday_names: Vec<&str> = if sunday { std::iter::once(WEEKDAYS[6]).chain(WEEKDAYS[..6].iter().copied()).collect() } else { WEEKDAYS.to_vec() };
            let cell = vec2(30.0, 28.0);
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            crate::theme::hrow(ui, |ui| {
                for name in weekday_names {
                    let (r, _) = ui.allocate_exact_size(cell, Sense::hover());
                    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, t(name), egui::FontId::proportional(12.0), ui.visuals().weak_text_color());
                }
            });
            let first = view.first_of_month();
            let offset = if sunday { first.weekday().to_sunday_zero_offset() } else { first.weekday().to_monday_zero_offset() } as i16;
            let days = first.days_in_month() as i16;
            for week in 0..6 {
                if week * 7 - offset >= days {
                    break;
                }
                crate::theme::hrow(ui, |ui| {
                    for col in 0..7 {
                        let day = week * 7 + col - offset + 1;
                        let (r, cell_resp) = ui.allocate_exact_size(cell, Sense::click());
                        if day < 1 || day > days {
                            continue;
                        }
                        let this = Date::new(view.year(), view.month(), day as i8).unwrap_or(view);
                        let selected = date == Some(this);
                        let visuals = ui.visuals();
                        let (fill, text) = if selected {
                            (theme::accent(), theme::on_accent())
                        } else if cell_resp.hovered() {
                            (visuals.widgets.hovered.bg_fill, visuals.strong_text_color())
                        } else {
                            (Color32::TRANSPARENT, visuals.text_color())
                        };
                        let radius = CornerRadius::same(6);
                        ui.painter().rect_filled(r, radius, fill);
                        if this == today && !selected {
                            ui.painter().rect_stroke(r, radius, egui::Stroke::new(1.5, theme::accent()), egui::StrokeKind::Inside);
                        }
                        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, day.to_string(), egui::FontId::proportional(13.0), text);
                        if cell_resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            *value = this.to_string();
                            changed = true;
                            ui.close();
                        }
                    }
                });
            }
            ui.separator();
            crate::theme::hrow(ui, |ui| {
                if ui.button(format!("📅  {}", t("Today"))).clicked() {
                    *value = today.to_string();
                    changed = true;
                    ui.close();
                }
                if ui.button(format!("✖  {}", t("Clear"))).clicked() {
                    value.clear();
                    changed = true;
                    ui.close();
                }
            });
            ui.data_mut(|d| {
                d.insert_temp(view_id, view);
                d.insert_temp(year_id, picking_year);
                d.insert_temp(month_id, picking_month);
            });
        });
    if !egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp)) {
        ui.data_mut(|d| {
            d.remove::<Date>(view_id);
            d.remove::<bool>(view_id.with("years"));
            d.remove::<bool>(view_id.with("months"));
        });
    }
    changed
}
