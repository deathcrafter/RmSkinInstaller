use clap::Parser;
use ini::Ini;
use ini::ParseOption;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitCode;
use uuid::Uuid;
use zip::ZipArchive;

const SHRT_MAX: usize = 32767;

#[derive(Debug)]
struct RainmeterSettings {
    skins_path: String,       // %USERPROFILE%\Documents\Rainmeter\Skins
    application_path: String, // %PROGRAMFILES%\Rainmeter
    settings_path: String,    // %APPDATA%\Rainmeter
}

#[derive(Debug)]
struct InstallOptions {
    was_running: bool,
    skinfile: String,
    temp_dir: String,
    plugins: Vec<String>,
    skins: Vec<String>,
    layouts: Vec<String>,
    variable_files: Vec<String>,
    merge_skins: bool,
    load_type: Option<String>,
    load: Option<String>,
}

#[derive(Parser, Debug)]
#[clap(
    name = "RmSkinInstaller",
    version = "0.0.0.0",
    author = "deathcrafter",
    long_about = "Command-line Rainmeter skin installer"
)]
struct Opts {
    #[arg(long)]
    skin: String,

    #[arg(long)]
    keepvariables: bool,

    #[arg(long)]
    nobackup: bool,
}

fn main() -> ExitCode {
    let opts = Opts::parse();

    if !Path::new(&opts.skin).is_file() {
        eprintln!("Skin file not found.");
        return ExitCode::FAILURE;
    }

    let mut install_options: InstallOptions = InstallOptions {
        was_running: false,
        skinfile: opts.skin.clone(),
        temp_dir: std::env::temp_dir().as_path().to_str().unwrap().to_owned()
            + Uuid::new_v4().to_string().as_str(),
        plugins: vec![],
        skins: vec![],
        layouts: vec![],
        variable_files: vec![],
        merge_skins: false,
        load_type: None,
        load: None,
    };

    // only support standard rainmeter installation
    let mut rainmeter_settings: RainmeterSettings = RainmeterSettings {
        skins_path: "".to_owned(),
        application_path: std::env::var("PROGRAMFILES").unwrap_or("".to_owned()) + "\\Rainmeter\\",
        settings_path: std::env::var("APPDATA").unwrap_or("".to_owned()) + "\\Rainmeter\\",
    };
    if !Path::new(&rainmeter_settings.application_path).exists()
        || !Path::new(&rainmeter_settings.settings_path)
            .join("Rainmeter.ini")
            .exists()
    {
        eprintln!("Rainmeter not installed or not run for the first time.");
        return ExitCode::FAILURE;
    }

    println!("Reading Rainmeter settings...");
    match read_rainmeter_settings(&mut rainmeter_settings) {
        Ok(_) => (),
        Err(e) => {
            eprintln!("Error reading Rainmeter settings: {}", e);
            return ExitCode::FAILURE;
        }
    }

    println!("Extracting skin to: {}", install_options.temp_dir);
    match extract_zip(&mut install_options) {
        Ok(_) => (),
        Err(e) => {
            eprintln!("Error extracting skin: {}", e);
            return ExitCode::FAILURE;
        }
    };

    println!("Reading skin options...");
    match read_options(&mut install_options) {
        Ok(_) => (),
        Err(e) => {
            eprintln!("Error reading options: {}", e);
            return ExitCode::FAILURE;
        }
    };

    // close rainmeter if running to start processing files
    println!("Closing Rainmeter if active...");
    if !close_rainmeter_if_running(&mut install_options.was_running) {
        eprintln!("Rainmeter is running. Please close Rainmeter before installing.");
        return ExitCode::FAILURE;
    }

    println!("Installing plugins...");
    match move_plugins(&mut install_options, &mut rainmeter_settings) {
        Ok(_) => (),
        Err(e) => {
            eprintln!("Error installing plugins: {}", e);
            return ExitCode::FAILURE;
        }
    };

    println!("Installing layouts...");
    match move_layouts(&mut install_options, &mut rainmeter_settings) {
        Ok(_) => (),
        Err(e) => {
            eprintln!("Error installing layouts: {}", e);
            return ExitCode::FAILURE;
        }
    };

    if install_options.merge_skins {
        println!("Merging skins...");

        if opts.keepvariables {
            println!("Keeping variables...");
            match keep_variables(&mut install_options, &mut rainmeter_settings) {
                Ok(_) => (),
                Err(e) => {
                    eprintln!("Error keeping variables: {}", e);
                    return ExitCode::FAILURE;
                }
            };
        }

        match merge_skins(&mut install_options, &mut rainmeter_settings) {
            Ok(_) => (),
            Err(e) => {
                eprintln!("Error merging skins: {}", e);
                return ExitCode::FAILURE;
            }
        };
    } else {
        println!("Restoring variables...");
        match keep_variables(&mut install_options, &mut rainmeter_settings) {
            Ok(_) => (),
            Err(e) => {
                eprintln!("Error keeping variables: {}", e);
                return ExitCode::FAILURE;
            }
        };

        if !opts.nobackup {
            println!("Creating backup...");
            match create_backup(&mut install_options, &mut rainmeter_settings) {
                Ok(_) => (),
                Err(e) => {
                    eprintln!("Error creating backup: {}", e);
                    return ExitCode::FAILURE;
                }
            };
        }

        println!("Installing skins...");
        match move_skins(&mut install_options, &mut rainmeter_settings) {
            Ok(_) => (),
            Err(e) => {
                eprintln!("Error installing skins: {}", e);
                return ExitCode::FAILURE;
            }
        };
    }

    start_rainmeter(&mut install_options, &mut rainmeter_settings);

    // cleanup
    println!("Cleaning up...");
    match fs::remove_dir_all(Path::new(&install_options.temp_dir)) {
        Ok(_) => (),
        Err(e) => {
            eprintln!("Error cleaning up: {}", e);
            return ExitCode::FAILURE;
        }
    };

    return ExitCode::SUCCESS;
}

// region Rainmeter process handler

fn close_rainmeter_if_running(was_running: &mut bool) -> bool {
    unsafe {
        let hwnd = windows::Win32::UI::WindowsAndMessaging::FindWindowW(
            windows::w!("DummyRainWClass"),
            windows::w!("Rainmeter control window"),
        );

        if hwnd == windows::Win32::Foundation::HWND(0) {
            return true;
        };

        *was_running = true;

        let mut process_id: u32 = 0;

        let _thread_id = windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            hwnd,
            Some(&mut process_id as *mut u32),
        );

        match windows::Win32::System::Threading::OpenProcess(
            windows::Win32::System::Threading::PROCESS_TERMINATE
                | windows::Win32::System::Threading::PROCESS_SYNCHRONIZE,
            false,
            process_id,
        ) {
            Ok(process_handle) => {
                windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    hwnd,
                    windows::Win32::UI::WindowsAndMessaging::WM_DESTROY,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );

                windows::Win32::System::Threading::WaitForSingleObject(process_handle, 5000);
                let mut lpexitcode: u32 = 0;
                windows::Win32::System::Threading::GetExitCodeProcess(
                    process_handle,
                    &mut lpexitcode as *mut u32,
                );
                windows::Win32::Foundation::CloseHandle(process_handle);

                if lpexitcode == 259u32 {
                    return false;
                }

                return true;
            }
            Err(e) => {
                println!("Error opening Rainmeter process: {}", e);
                return false;
            }
        }
    }
}

fn start_rainmeter(
    install_options: &mut InstallOptions,
    rainmeter_settings: &mut RainmeterSettings,
) {
    let mut rainmeter_exe = rainmeter_settings.application_path.clone();
    rainmeter_exe.push_str("Rainmeter.exe");

    let mut command = Command::new("powershell");
    command.arg("Start-Process");
    command.arg("\"".to_owned() + &rainmeter_exe + "\"");

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("Error starting Rainmeter: {}", e);
            return;
        }
    };

    match child.wait() {
        Ok(_) => (),
        Err(e) => {
            eprintln!("Error waiting for Rainmeter to start: {}", e);
            return;
        }
    };

    std::thread::sleep(std::time::Duration::from_millis(1000)); // wait for a for rainmeter to start

    if install_options.load_type == None {
        return;
    }

    command.arg("-ArgumentList");
    if install_options.load_type.as_ref().unwrap() == "Skin" {
        let skin = install_options.load.as_mut().unwrap().rfind('\\').unwrap();
        let (skin, file) = install_options.load.as_mut().unwrap().split_at(skin + 1);
        command.arg("@('[!ActivateConfig \"".to_owned() + skin + "\" \"" + file + "\"]')");
    } else if install_options.load_type.as_ref().unwrap() == "Layout" {
        command.arg(
            "@('[!LoadLayout \"".to_owned()
                + install_options.load.as_mut().unwrap().as_str()
                + "\"]')",
        );
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("Error starting Rainmeter with commands: {}", e);
            return;
        }
    };

    match child.wait() {
        Ok(_) => (),
        Err(e) => {
            eprintln!("Error waiting for Rainmeter to start with commands: {}", e);
            return;
        }
    };
}

// endregion

// region config parsers

fn read_rainmeter_settings(
    rainmeter_settings: &mut RainmeterSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    let settings_filepath = Path::new(rainmeter_settings.settings_path.as_str())
        .join("Rainmeter.ini")
        .to_str()
        .unwrap()
        .to_owned();

    let settings = match read_ini(settings_filepath.as_str()) {
        Ok(ini) => ini,
        Err(e) => {
            println!("Error opening settings file: {}", e);
            return Err(Box::new(e));
        }
    };

    let rainmeter = settings.section(Some("Rainmeter")).unwrap();

    rainmeter_settings.skins_path = match rainmeter.get("SkinPath") {
        Some(skin_path) => skin_path.to_owned(),
        None => {
            std::env::var("USERPROFILE").unwrap_or("".to_owned())
                + "\\Documents\\Rainmeter\\Skins\\"
        }
    };

    if !Path::new(rainmeter_settings.skins_path.as_str()).is_dir() {
        if fs::create_dir_all(rainmeter_settings.skins_path.as_str()).is_err() {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Rainmeter skins folder not found and could not be created.",
            )));
        }
    }

    Ok(())
}

fn read_options(install_options: &mut InstallOptions) -> Result<(), Box<dyn std::error::Error>> {
    let settings_filepath = Path::new(install_options.temp_dir.as_str()).join("RMSKIN.ini");

    let settings = match read_ini(settings_filepath.to_str().unwrap()) {
        Ok(ini) => ini,
        Err(_) => {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "RMSKIN.ini not found.",
            )));
        }
    };

    let rmskin = settings.section(Some("rmskin")).unwrap();

    install_options.load_type = match rmskin.get("LoadType") {
        Some(load_type) => Some(load_type.to_owned()),
        None => None,
    };
    install_options.load = match rmskin.get("Load") {
        Some(load) => Some(load.to_owned()),
        None => None,
    };
    install_options.variable_files = match rmskin.get("VariableFiles") {
        Some(skins) => skins.split(" | ").map(|s| s.to_owned()).collect(),
        None => vec![],
    };
    install_options.merge_skins = match rmskin.get("MergeSkins") {
        Some(merge_skins) => merge_skins == "1",
        None => false,
    };

    Ok(())
}

// endregion

// #region file operations

fn extract_zip(install_options: &mut InstallOptions) -> Result<(), Box<dyn std::error::Error>> {
    let zip = install_options.skinfile.as_str();

    // if the temppath exists, delete it
    if Path::new(install_options.temp_dir.as_str()).is_dir() {
        if (fs::remove_dir_all(Path::new(install_options.temp_dir.as_str()))).is_err() {
            println!("Error removing directory");
        }
    }
    if fs::DirBuilder::new()
        .recursive(true)
        .create(Path::new(install_options.temp_dir.as_str()))
        .is_err()
    {
        println!(
            "Error creating directory: {}",
            install_options.temp_dir.as_str()
        );
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Error creating directory",
        )));
    }

    let zip_path: &Path = Path::new(zip);
    let zip_file: fs::File = match fs::File::open(zip_path) {
        Ok(file) => file,
        Err(e) => {
            println!("Error opening zip file: {}", e);
            return Err(Box::new(e));
        }
    };

    let mut archive: ZipArchive<fs::File> = match ZipArchive::new(zip_file) {
        Ok(archive) => archive,
        Err(e) => {
            println!("Error reading zip file: {}", e);
            return Err(Box::new(e));
        }
    };

    let outpath = PathBuf::from(install_options.temp_dir.as_str());

    let mut found_rmskin = false;

    // iterate over the contents of the zip file
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();

        let mut outfilename: PathBuf = match file.enclosed_name() {
            Some(path) => path.to_owned(),
            None => continue,
        };

        let (component, name, extension) = parse_zip_item(&outfilename.to_str().unwrap());
        if component.eq("Skins") {
            if !name.is_empty() {
                install_options.skins.push(name.to_owned());
            }
        }
        if component.eq("Layouts") {
            if !name.is_empty() {
                install_options.layouts.push(name.to_owned());
            }
        }
        if component.eq("Plugins") {
            if name.eq("64bit") && extension.eq("dll") {
                let plugin_name = outfilename
                    .to_str()
                    .unwrap()
                    .split(std::path::MAIN_SEPARATOR)
                    .last()
                    .unwrap()
                    .to_owned();
                install_options.plugins.push(plugin_name);
            } else {
                // don't process 32bit plugins
                continue;
            }
        }
        if name.eq("RMSKIN.ini") {
            found_rmskin = true;
        }

        outfilename = outpath.join(outfilename);

        if outfilename
            .to_str()
            .unwrap()
            .ends_with(std::path::MAIN_SEPARATOR.to_string().as_str())
        {
            match fs::DirBuilder::new().recursive(true).create(outfilename) {
                Ok(_) => (),
                Err(e) => {
                    println!("Error creating directory: {}", e);
                    return Err(Box::new(e));
                }
            };
            continue;
        }

        match fs::DirBuilder::new()
            .recursive(true)
            .create(outfilename.parent().unwrap())
        {
            Ok(_) => (),
            Err(e) => {
                println!("Error creating directory: {}", e);
                return Err(Box::new(e));
            }
        };

        let mut outfile = match fs::File::create(&outfilename) {
            Ok(file) => file,
            Err(e) => {
                println!("Error creating file: {}", e);
                return Err(Box::new(e));
            }
        };

        match std::io::copy(&mut file, &mut outfile) {
            Ok(_) => (),
            Err(e) => {
                println!("Error extracting file: {}", outfilename.to_str().unwrap());
                return Err(Box::new(e));
            }
        };
    }

    if !found_rmskin {
        println!("Error: RMSKIN.ini not found in zip");
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "RMSKIN.ini not found in zip",
        )));
    }

    install_options.skins.sort();
    install_options.skins.dedup();
    install_options.layouts.sort();
    install_options.layouts.dedup();
    install_options.plugins.sort();
    install_options.plugins.dedup();

    return Ok(());
}

fn keep_variables(
    install_options: &mut InstallOptions,
    rainmeter_settings: &mut RainmeterSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    for varfile in &install_options.variable_files[..] {
        let oldfile = Path::new(&rainmeter_settings.skins_path).join(Path::new(&varfile));
        let newfile = Path::new(&install_options.temp_dir)
            .join("Skins")
            .join(Path::new(&varfile));

        if !oldfile.is_file() {
            continue;
        }

        if !newfile.is_file() {
            if fs::File::create(&newfile).is_err() {
                continue;
            }
        }

        let mut keys: Vec<Vec<u16>> = vec![];
        let mut values: Vec<Vec<u16>> = vec![];

        read_win_ini(&oldfile, &mut keys, &mut values);

        unsafe {
            let appname = windows::core::HSTRING::from("Variables");
            let filename = windows::core::HSTRING::from(newfile.to_str().unwrap());
            let mut i = 0;
            while i < keys.len() {
                let key = windows::core::PCWSTR(keys[i].as_ptr());
                let value = windows::core::PCWSTR(values[i].as_ptr());
                windows::Win32::System::WindowsProgramming::WritePrivateProfileStringW(
                    &appname, key, value, &filename,
                );
                i += 1;
            }
        }
    }

    Ok(())
}

// todo: add version check
fn move_plugins(
    install_options: &mut InstallOptions,
    rainmeter_settings: &mut RainmeterSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    let oldfile = Path::new(&install_options.temp_dir).join("Plugins\\64bit");
    let newfile = Path::new(&rainmeter_settings.settings_path).join("Plugins");

    copy_dir_all(&oldfile, &newfile)?;
    Ok(())
}

fn move_layouts(
    install_options: &mut InstallOptions,
    rainmeter_settings: &mut RainmeterSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    let oldfile = Path::new(&install_options.temp_dir).join("Layouts");
    let newfile = Path::new(&rainmeter_settings.settings_path).join("Layouts");

    copy_dir_all(&oldfile, &newfile)?;
    Ok(())
}

fn create_backup(
    install_options: &mut InstallOptions,
    rainmeter_settings: &mut RainmeterSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    let backup_dir = Path::new(&rainmeter_settings.skins_path).join("@Backup");
    if !backup_dir.is_dir() {
        match fs::create_dir_all(&backup_dir) {
            Ok(_) => (),
            Err(e) => {
                println!("Error creating backup directory: {}", e);
                return Err(Box::new(e));
            }
        }
    }
    for skin in &install_options.skins[..] {
        let oldfile = Path::new(&rainmeter_settings.skins_path).join(Path::new(&skin));
        let newfile = Path::new(&backup_dir).join(Path::new(&skin));

        match copy_dir_all(&oldfile, &newfile) {
            Ok(_) => (),
            Err(e) => {
                println!("Error moving file to backup: {}", oldfile.to_str().unwrap());
                return Err(e);
            }
        };

        if oldfile.is_dir() {
            match fs::remove_dir_all(&oldfile) {
                Ok(_) => (),
                Err(e) => {
                    println!("Error removing directory: {}", oldfile.to_str().unwrap());
                    return Err(Box::new(e));
                }
            }
        }
    }

    Ok(())
}

fn move_skins(
    install_options: &mut InstallOptions,
    rainmeter_settings: &mut RainmeterSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    for skin in &install_options.skins[..] {
        let oldfile = Path::new(&install_options.temp_dir)
            .join("Skins")
            .join(Path::new(&skin));
        let newfile = Path::new(&rainmeter_settings.skins_path).join(Path::new(&(skin.to_owned())));

        match copy_dir_all(&oldfile, &newfile) {
            Ok(_) => (),
            Err(e) => {
                return Err(e);
            }
        }
    }
    Ok(())
}

fn merge_skins(
    install_options: &mut InstallOptions,
    rainmeter_settings: &mut RainmeterSettings,
) -> Result<(), Box<dyn std::error::Error>> {
    for skin in &install_options.skins[..] {
        let newfile = Path::new(&rainmeter_settings.skins_path).join(Path::new(&skin));
        let oldfile = Path::new(&install_options.temp_dir)
            .join("Skins")
            .join(Path::new(&skin));

        match copy_dir_all(&oldfile, &newfile) {
            Ok(_) => (),
            Err(e) => {
                println!("Error merging file: {}", oldfile.to_str().unwrap());
                return Err(e);
            }
        };
    }
    Ok(())
}

// #endregion

// #region helper functions

fn parse_zip_item(item: &str) -> (String, String, String) {
    let mut component: String = "".to_owned();
    let mut name: String = "".to_owned();
    let mut extension: String = "".to_owned();

    let split = item.split(std::path::MAIN_SEPARATOR).collect::<Vec<&str>>();
    if split.len() > 2 {
        component = split[0].to_owned();
        name = split[1].to_owned();
        extension = Path::new(split.last().unwrap())
            .extension()
            .unwrap_or(OsStr::new(""))
            .to_str()
            .unwrap()
            .to_owned();
    } else if split.len() > 1 {
        name = split[0].to_owned();
        extension = Path::new(split[1])
            .extension()
            .unwrap_or(OsStr::new(""))
            .to_str()
            .unwrap()
            .to_owned();
    } else if split.len() > 0 {
        name = split[0].to_owned();
    }

    return (component, name, extension);
}

// uses the GetPrivateProfileSectionW function to read a win ini file
// https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getprivateprofilesectionw
// reads the ini key and values to u16 vectors, since rust conversions are lossy
fn read_win_ini(file_path: &Path, keys: &mut Vec<Vec<u16>>, values: &mut Vec<Vec<u16>>) {
    unsafe {
        let appname = windows::core::HSTRING::from("Variables");
        let mut section = vec![0u16; SHRT_MAX];
        let filename = windows::core::HSTRING::from(file_path.to_str().unwrap());

        windows::Win32::System::WindowsProgramming::GetPrivateProfileSectionW(
            &appname,
            Some(&mut section),
            &filename,
        );

        let mut start = 0;
        let mut end = 0;
        let mut was_null = false;
        for c in section.iter() {
            if std::char::from_u32(*c as u32) == Some('=') {
                was_null = false;
                keys.push(section[start..end].to_vec());
                keys.last_mut().unwrap().push(0);
                end = end + 1;
                start = end;
            } else if std::char::from_u32(*c as u32) == Some('\0') {
                if was_null {
                    break;
                }
                was_null = true;
                values.push(section[start..(end + 1)].to_vec());
                end = end + 1;
                start = end;
            } else {
                was_null = false;
                end += 1;
            }
        }
    }
}

fn read_ini(file_path: &str) -> Result<Ini, Box<ini::ParseError>> {
    let ini_path = Path::new(file_path);
    if !ini_path.is_file() {
        eprintln!("Error: {} is not a file", file_path);
        return Err(Box::new(ini::ParseError {
            line: 430,
            col: 0,
            msg: "Not a file".to_owned(),
        }));
    }

    // try utf-8 first
    return match Ini::load_from_file_opt(
        ini_path,
        ParseOption {
            enabled_quote: false,
            enabled_escape: false,
        },
    ) {
        Ok(ini) => Ok(ini),
        // if utf-8 fails, try utf-16
        Err(e) => {
            let content = utf16_reader::read_to_string(std::io::BufReader::new(
                match std::fs::File::open(file_path) {
                    Ok(file) => file,
                    Err(_) => {
                        eprintln!("Error opening ini file: {}", e);
                        return Err(Box::new(ini::ParseError {
                            line: 452,
                            col: 13,
                            msg: "Error opening file".to_owned(),
                        }));
                    }
                },
            ));
            match Ini::load_from_str_opt(
                &content,
                ParseOption {
                    enabled_quote: false,
                    enabled_escape: false,
                },
            ) {
                Ok(ini) => Ok(ini),
                Err(e) => {
                    eprintln!("Error parsing ini file: {}", e);
                    return Err(Box::new(e));
                }
            }
        }
    };
}

fn copy_dir_all(src: &Path, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !src.is_dir() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Source is not a directory",
        )));
    }

    if dest.is_file() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Destination is a file",
        )));
    }

    if !dest.is_dir() {
        match fs::create_dir_all(dest) {
            Ok(_) => (),
            Err(e) => {
                return Err(Box::new(e));
            }
        }
    }

    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let dest_path = dest.join(path.file_name().unwrap());
        if path.is_dir() {
            match copy_dir_all(&path, &dest_path) {
                Ok(_) => (),
                Err(e) => {
                    return Err(e);
                }
            }
        } else {
            if !dest_path.parent().unwrap().is_dir() {
                match fs::create_dir_all(&dest_path) {
                    Ok(_) => (),
                    Err(e) => {
                        return Err(Box::new(e));
                    }
                }
            }
            match fs::copy(&path, &dest_path) {
                Ok(_) => (),
                Err(e) => {
                    return Err(Box::new(e));
                } // ignore copy errors
            }
        }
    }

    Ok(())
}

fn _move_dir_all(src: &Path, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !src.is_dir() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Source is not a directory",
        )));
    }

    if dest.is_file() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Destination is a file",
        )));
    }

    if !dest.is_dir() {
        match fs::create_dir_all(dest) {
            Ok(_) => (),
            Err(e) => {
                return Err(Box::new(e));
            }
        }
    }

    for entry in fs::read_dir(&src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let dest_path = dest.join(path.file_name().unwrap());
        if path.is_dir() {
            match _move_dir_all(&path, &dest_path) {
                Ok(_) => (),
                Err(e) => {
                    return Err(e);
                }
            }
        } else {
            if !dest_path.parent().unwrap().is_dir() {
                match fs::create_dir_all(&dest_path) {
                    Ok(_) => (),
                    Err(e) => {
                        return Err(Box::new(e));
                    }
                }
            }
            match fs::rename(&path, &dest_path) {
                Ok(_) => (),
                Err(e) => {
                    return Err(Box::new(e));
                }
            }
        }
    }
    Ok(())
}

// #endregion
