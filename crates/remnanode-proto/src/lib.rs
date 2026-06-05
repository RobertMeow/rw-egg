pub mod xray {
    pub mod app {
        pub mod proxyman {
            pub mod command {
                include!(concat!(env!("OUT_DIR"), "/xray.app.proxyman.command.rs"));
            }
        }
        pub mod stats {
            pub mod command {
                include!(concat!(env!("OUT_DIR"), "/xray.app.stats.command.rs"));
            }
        }
        pub mod router {
            pub mod command {
                include!(concat!(env!("OUT_DIR"), "/xray.app.router.command.rs"));
            }
        }
    }
    pub mod common {
        pub mod net {
            include!(concat!(env!("OUT_DIR"), "/xray.common.net.rs"));
        }
        pub mod protocol {
            include!(concat!(env!("OUT_DIR"), "/xray.common.protocol.rs"));
        }
        pub mod serial {
            include!(concat!(env!("OUT_DIR"), "/xray.common.serial.rs"));
        }
    }
    pub mod core {
        include!(concat!(env!("OUT_DIR"), "/xray.core.rs"));
    }
}
