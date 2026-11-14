fn main() -> eframe::Result {
    #[cfg(not(target_os = "android"))]
    {
        openmix::run_desktop()
    }
    #[cfg(target_os = "android")]
    {
        Ok(())
    }
}
