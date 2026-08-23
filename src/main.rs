use dynon_usb_updater::scan;

fn main() {
    println!("{}", scan::parse_cycle("airmate_av_data_us_2608_013712.dup").unwrap());
}
