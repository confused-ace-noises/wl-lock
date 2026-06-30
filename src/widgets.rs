use std::time::Duration;
use chrono::Local;
use egui::{RichText, Widget};

pub struct Clock {
    pub time_style: Box<dyn Fn(RichText) -> RichText>,
    pub date_style: Box<dyn Fn(RichText) -> RichText>,
}

impl Clock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn time_style(mut self, f: impl Fn(RichText) -> RichText + 'static) -> Self {
        self.time_style = Box::new(f);
        self
    }

    pub fn date_style(mut self, f: impl Fn(RichText) -> RichText + 'static) -> Self {
        self.date_style = Box::new(f);
        self
    }
}

impl Widget for Clock {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        ui.ctx().request_repaint_after(Duration::from_secs(60));

        let now = Local::now();
        let date = now.format("%A %_d/%m/%y").to_string();
        let time = now.format("%R").to_string();

        ui.vertical_centered(|ui| {
            ui.label((self.time_style)(RichText::new(time)));
            ui.label((self.date_style)(RichText::new(date)));
        }).response
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            time_style: Box::new(|t| t.size(30.0)),
            date_style: Box::new(|t| t.size(10.0)),
        }
    }
}