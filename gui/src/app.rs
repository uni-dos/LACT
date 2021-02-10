mod apply_revealer;
mod header;
mod root_stack;

extern crate gtk;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use apply_revealer::ApplyRevealer;
use daemon::daemon_connection::DaemonConnection;
use daemon::gpu_controller::GpuStats;
use daemon::DaemonError;
use gtk::*;

use header::Header;
use root_stack::RootStack;

#[derive(Clone)]
pub struct App {
    pub window: Window,
    pub header: Header,
    root_stack: RootStack,
    apply_revealer: ApplyRevealer,
    daemon_connection: DaemonConnection,
}

impl App {
    pub fn new(daemon_connection: DaemonConnection) -> Self {
        let window = Window::new(WindowType::Toplevel);

        let header = Header::new();

        window.set_titlebar(Some(&header.container));
        window.set_title("LACT");

        window.set_default_size(500, 600);

        window.connect_delete_event(move |_, _| {
            main_quit();
            Inhibit(false)
        });

        let root_stack = RootStack::new();

        header.set_switcher_stack(&root_stack.container);

        let root_box = Box::new(Orientation::Vertical, 5);

        root_box.add(&root_stack.container);

        let apply_revealer = ApplyRevealer::new();

        root_box.add(&apply_revealer.container);

        window.add(&root_box);

        App {
            window,
            header,
            root_stack,
            apply_revealer,
            daemon_connection,
        }
    }

    pub fn run(&self) -> Result<(), DaemonError> {
        self.window.show_all();

        let current_gpu_id = Arc::new(AtomicU32::new(0));

        {
            let current_gpu_id = current_gpu_id.clone();
            let app = self.clone();

            self.header.connect_gpu_selection_changed(move |gpu_id| {
                log::info!("GPU Selection changed");
                app.set_info(gpu_id);
                current_gpu_id.store(gpu_id, Ordering::SeqCst);
            });
        }

        let gpus = self.daemon_connection.get_gpus()?;

        self.header.set_gpus(gpus);

        // Show apply button on setting changes
        {
            let apply_revealer = self.apply_revealer.clone();

            self.root_stack
                .thermals_page
                .connect_settings_changed(move || {
                    log::info!("Settings changed, showing apply button");
                    apply_revealer.show();
                });

            let apply_revealer = self.apply_revealer.clone();

            self.root_stack.oc_page.connect_settings_changed(move || {
                log::info!("Settings changed, showing apply button");
                apply_revealer.show();
            });
        }

        // Apply settings
        {
            let current_gpu_id = current_gpu_id.clone();
            let app = self.clone();

            self.apply_revealer.connect_apply_button_clicked(move || {
                log::info!("Applying settings");

                let gpu_id = current_gpu_id.load(Ordering::SeqCst);

                {
                    let thermals_settings = app.root_stack.thermals_page.get_thermals_settings();

                    if thermals_settings.automatic_fan_control_enabled {
                        app.daemon_connection
                            .stop_fan_control(gpu_id)
                            .expect("Failed to stop fan control");
                    } else {
                        app.daemon_connection
                            .start_fan_control(gpu_id)
                            .expect("Failed to start fan control");
                    }

                    app.daemon_connection
                        .set_fan_curve(gpu_id, thermals_settings.curve)
                        .expect("Failed to set fan curve");
                }

                {
                    let power_profile = app.root_stack.oc_page.get_power_profile();

                    if let Some(profile) = power_profile {
                        app.daemon_connection
                            .set_power_profile(gpu_id, profile)
                            .expect("Failed to set power profile");
                    }
                }

                app.set_info(gpu_id);
            });
        }

        self.start_stats_update_loop(current_gpu_id.clone());

        Ok(gtk::main())
    }

    fn set_info(&self, gpu_id: u32) {
        let gpu_info = self.daemon_connection.get_gpu_info(gpu_id).unwrap();
        log::trace!("Setting info {:?}", &gpu_info);

        self.root_stack.info_page.set_info(&gpu_info);

        log::trace!("Setting power profile {:?}", gpu_info.power_profile);
        self.root_stack
            .oc_page
            .set_power_profile(&gpu_info.power_profile);

        log::trace!("Setting fan control info");
        match self.daemon_connection.get_fan_control(gpu_id) {
            Ok(fan_control_info) => self
                .root_stack
                .thermals_page
                .set_ventilation_info(fan_control_info),
            Err(_) => self.root_stack.thermals_page.hide_fan_controls(),
        }

        self.apply_revealer.hide();
    }

    fn start_stats_update_loop(&self, current_gpu_id: Arc<AtomicU32>) {
        // The loop that gets stats
        let (sender, receiver) = glib::MainContext::channel(glib::PRIORITY_DEFAULT);
        {
            let daemon_connection = self.daemon_connection.clone();

            thread::spawn(move || loop {
                let gpu_id = current_gpu_id.load(Ordering::SeqCst);

                if let Ok(stats) = daemon_connection.get_gpu_stats(gpu_id) {
                    sender.send(GuiUpdateMsg::GpuStats(stats)).unwrap();
                }

                thread::sleep(Duration::from_millis(500));
            });
        }

        // Receiving stats into the gui event loop
        {
            let thermals_page = self.root_stack.thermals_page.clone();
            let oc_page = self.root_stack.oc_page.clone();

            receiver.attach(None, move |msg| {
                match msg {
                    GuiUpdateMsg::GpuStats(stats) => {
                        log::trace!("New stats received, updating");
                        thermals_page.set_thermals_info(&stats);
                        oc_page.set_stats(&stats);
                    } /*GuiUpdateMsg::FanControlInfo(fan_control_info) => {
                          thermals_page.set_ventilation_info(fan_control_info)
                      }*/
                }

                glib::Continue(true)
            });
        }
    }
}

enum GuiUpdateMsg {
    // FanControlInfo(FanControlInfo),
    GpuStats(GpuStats),
}
