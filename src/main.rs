#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        // Reuse a terminal for help, setup and errors, but never allocate a console.
        // No parent console (Explorer/launcher) or already attached is normal.
        // STARTF_USESTDHANDLES preserves the launcher's redirected log pipes.
        // SAFETY: ATTACH_PARENT_PROCESS is a valid DWORD; there are no pointers.
        unsafe { AttachConsole(u32::MAX) };
    }
    if let Err(error) = krasnokamen_client::cli(std::env::args().skip(1)) {
        eprintln!("krasnokamen-client: {error}");
        std::process::exit(1);
    }
}
