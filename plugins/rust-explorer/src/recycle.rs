//! A native Rust view of the Windows Recycle Bin. Windows owns the storage,
//! restore metadata and file operations; this module never parses $I files.
use super::*;
use windows::{core::{Interface,PCWSTR,PWSTR,GUID}, Win32::{Foundation::{PROPERTYKEY,HWND as WHWND},System::Com::*,UI::Shell::*}};
use windows::Win32::UI::Shell::{SHCreateItemFromParsingName,SHGetKnownFolderItem,FOLDERID_RecycleBinFolder,KF_FLAG_DEFAULT,FMTID_Displaced,PID_DISPLACED_FROM,PID_DISPLACED_DATE,SIGDN_DESKTOPABSOLUTEPARSING,SIGDN_NORMALDISPLAY,BHID_EnumItems,FileOperation,SHEmptyRecycleBinW,FOF_RENAMEONCOLLISION,FOF_NOCONFIRMATION,FOF_NOERRORUI,SHERB_NOCONFIRMATION,SHERB_NOPROGRESSUI};
use windows::Win32::System::SystemServices::SFGAO_FOLDER;
use std::sync::{Mutex,atomic::{AtomicBool,Ordering}};

pub const PATH:&str="shell:RecycleBinFolder";
pub const DONE:u32=WM_APP+22;
pub const RESTORE:usize=31;
pub const DELETE:usize=32;
pub const EMPTY:usize=33;
const FILE_NAME:PROPERTYKEY=PROPERTYKEY{fmtid:GUID::from_u128(0x41cf5ae0_f75a_4806_bd87_59c7d9248eb9),pid:100};
const SIZE_KEY:PROPERTYKEY=PROPERTYKEY{fmtid:GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac),pid:12};
#[derive(Clone,Debug)]
pub struct Entry{pub id:String,pub name:String,pub original_dir:String}
pub fn is(path:&Path)->bool{path.as_os_str().eq_ignore_ascii_case(std::ffi::OsStr::new(PATH))}
pub fn current()->bool{is(&cur_dir())}
unsafe fn owned(p:PWSTR)->String {let text=unsafe{p.to_string().unwrap_or_default()};unsafe{CoTaskMemFree(Some(p.0.cast()));}text}
unsafe fn shell_item(id:&str)->windows::core::Result<IShellItem>{unsafe{SHCreateItemFromParsingName(PCWSTR(w(id).as_ptr()),None)}}

pub fn list(cancelled:&impl Fn()->bool)->Result<Vec<Row>,String>{unsafe{
    (||->windows::core::Result<Vec<Row>>{
        let bin:IShellItem=SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder,KF_FLAG_DEFAULT,None)?;
        let enumeration:IEnumShellItems=bin.BindToHandler(None,&BHID_EnumItems)?;
        let from=PROPERTYKEY{fmtid:FMTID_Displaced,pid:PID_DISPLACED_FROM};
        let date=PROPERTYKEY{fmtid:FMTID_Displaced,pid:PID_DISPLACED_DATE};
        let mut rows=vec![];
        while !cancelled(){
            let mut slot=[None];let mut fetched=0;
            enumeration.Next(&mut slot,Some(&mut fetched))?;
            if fetched==0{break;}
            let Some(item)=slot[0].take()else{break};
            let prop:IShellItem2=item.cast()?;
            let id=owned(item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)?);
            let name=prop.GetString(&FILE_NAME).map(|s|owned(s)).unwrap_or_else(|_|item.GetDisplayName(SIGDN_NORMALDISPLAY).map(|s|owned(s)).unwrap_or_else(|_|id.clone()));
            let original_dir=prop.GetString(&from).map(|s|owned(s)).unwrap_or_default();
            let time=prop.GetFileTime(&date).map_or(0,|f|(f.dwHighDateTime as u64)<<32|f.dwLowDateTime as u64);
            let size=prop.GetUInt64(&SIZE_KEY).unwrap_or(0);
            let dir=item.GetAttributes(SFGAO_FOLDER).is_ok_and(|a|a.0&SFGAO_FOLDER.0!=0);
            rows.push(Row{name:id.clone(),dir,size,time,icon:-1,thumb:-2,hdr:false,recycled:Some(Entry{id,name,original_dir})});
        }
        Ok(rows)
    })().map_err(|e|e.to_string())
}}

pub fn watch_dirs()->Vec<PathBuf>{
    let drives=unsafe{windows_sys::Win32::Storage::FileSystem::GetLogicalDrives()};
    (0..26).filter(|i|drives&(1<<i)!=0).map(|i|PathBuf::from(format!("{}:\\$Recycle.Bin",(b'A'+i as u8)as char))).collect()
}

pub fn selected_entries()->Vec<Entry>{
    ST.with_borrow(|s|{let mut entries=vec![];let mut i=-1;loop{i=put(s.list,LVM_GETNEXTITEM,i as usize,LVNI_SELECTED as isize);let Some(r)=usize::try_from(i).ok().and_then(|i|s.rows.get(i))else{break};if let Some(e)=&r.recycled{entries.push(e.clone());}}entries})
}
static BUSY:AtomicBool=AtomicBool::new(false);
static RESULT:Mutex<Option<Result<(),String>>>=Mutex::new(None);
pub fn busy()->bool{BUSY.load(Ordering::Acquire)}

pub fn run(h:HWND,action:usize,entries:Vec<Entry>){
    if busy() || (action!=EMPTY && entries.is_empty()){return;}
    if action==DELETE || action==EMPTY {
        let text=if action==EMPTY{"Очистить корзину? Все находящиеся в ней файлы будут удалены навсегда."}else{"Удалить выбранные файлы навсегда? Восстановить их из корзины будет нельзя."};
        if unsafe{MessageBoxW(h,w(text).as_ptr(),w("Корзина WinRust").as_ptr(),MB_YESNO|MB_ICONWARNING|MB_DEFBUTTON2)}!=IDYES{return;}
    }
    if BUSY.swap(true,Ordering::AcqRel){return;}
    ui::activity(Some("Выполнение операции с корзиной…".into()));unsafe{ui::status_dirty(h);}
    let window=h as isize;
    std::thread::spawn(move||{
        let result=unsafe{
            let initialized=CoInitializeEx(None,COINIT_APARTMENTTHREADED).is_ok();
            let result=operate(window,action,&entries).map_err(|e|e.to_string());
            if initialized{CoUninitialize();}result
        };
        *RESULT.lock().unwrap()=Some(result);
        unsafe{PostMessageW(window as HWND,DONE,0,0);}
    });
}
pub fn take_result()->Option<Result<(),String>>{let result=RESULT.lock().unwrap().take();if result.is_some(){BUSY.store(false,Ordering::Release);}result}

unsafe fn operate(window:isize,action:usize,entries:&[Entry])->windows::core::Result<()>{unsafe{
    if action==EMPTY{return SHEmptyRecycleBinW(Some(WHWND(window as _)),PCWSTR::null(),SHERB_NOCONFIRMATION|SHERB_NOPROGRESSUI);}
    let op:IFileOperation=CoCreateInstance(&FileOperation,None,CLSCTX_ALL)?;
    op.SetOwnerWindow(WHWND(window as _))?;
    op.SetOperationFlags(FOF_RENAMEONCOLLISION|FOF_NOCONFIRMATION|FOF_NOERRORUI)?;
    let bin:IShellItem=SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder,KF_FLAG_DEFAULT,None)?;
    let enumeration:IEnumShellItems=bin.BindToHandler(None,&BHID_EnumItems)?;
    let mut found=std::collections::HashMap::new();
    loop{let mut slot=[None];let mut count=0;enumeration.Next(&mut slot,Some(&mut count))?;if count==0{break;}let Some(item)=slot[0].take()else{break};let id=owned(item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)?);if entries.iter().any(|e|e.id==id){found.insert(id,item);}}
    for entry in entries{
        let item=found.remove(&entry.id).ok_or_else(||windows::core::Error::from_hresult(windows::core::HRESULT(0x80070002u32 as i32)))?;
        if action==RESTORE{
            if entry.original_dir.is_empty()||Path::new(&entry.name).file_name()!=Some(std::ffi::OsStr::new(&entry.name)){return Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x80070057u32 as i32)));}
            fs::create_dir_all(&entry.original_dir).map_err(|e|windows::core::Error::from(std::io::Error::new(e.kind(),e.to_string())))?;
            let destination=shell_item(&entry.original_dir)?;
            op.MoveItem(&item,&destination,PCWSTR(w(&entry.name).as_ptr()),None)?;
        }else{op.DeleteItem(&item,None)?;}
    }
    op.PerformOperations()?;if op.GetAnyOperationsAborted()?.as_bool(){return Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x800704c7u32 as i32)));}
    Ok(())
}}

pub unsafe fn menu(h:HWND,selected:bool){unsafe{
    let menu=CreatePopupMenu();
    if selected{AppendMenuW(menu,MF_STRING,RESTORE,w("Восстановить выбранное").as_ptr());AppendMenuW(menu,MF_STRING,DELETE,w("Удалить навсегда…").as_ptr());}
    AppendMenuW(menu,MF_STRING,EMPTY,w("Очистить корзину…").as_ptr());
    AppendMenuW(menu,MF_STRING,34,w("Обновить").as_ptr());
    let mut pt:POINT=std::mem::zeroed();GetCursorPos(&mut pt);
    let action=TrackPopupMenu(menu,TPM_RETURNCMD|TPM_RIGHTBUTTON,pt.x,pt.y,0,h,ptr::null())as usize;DestroyMenu(menu);
    if action==34{reload(h);}else if action!=0{run(h,action,selected_entries());}
}}

#[cfg(test)]
mod tests{
    use windows::Win32::UI::Shell::{FOF_ALLOWUNDO,FOF_SILENT};
    use super::*;
    #[test]
    #[ignore="read-only validation of installed Unicode shortcuts"]
    fn installed_recycle_shortcuts_target_native_view(){unsafe{
        use windows::Win32::{System::Com::{IPersistFile,STGM},UI::Shell::{IShellLinkW,ShellLink,FOLDERID_Desktop,FOLDERID_Programs}};
        let initialized=CoInitializeEx(None,COINIT_APARTMENTTHREADED).is_ok();
        for(folder,name)in [(&FOLDERID_Desktop,"Корзина.lnk"),(&FOLDERID_Programs,"Корзина (Recycle Bin).lnk")]{
            let path=sh::known_dir(folder).unwrap().join(name);assert!(path.is_file());let link:IShellLinkW=CoCreateInstance(&ShellLink,None,CLSCTX_ALL).unwrap();link.cast::<IPersistFile>().unwrap().Load(PCWSTR(w(&path.to_string_lossy()).as_ptr()),STGM(0)).unwrap();
            let mut target=[0u16;1024];link.GetPath(&mut target,ptr::null_mut(),0).unwrap();let target=String::from_utf16_lossy(&target[..target.iter().position(|&c|c==0).unwrap()]);
            let mut args=[0u16;256];link.GetArguments(&mut args).unwrap();let args=String::from_utf16_lossy(&args[..args.iter().position(|&c|c==0).unwrap()]);
            assert_eq!(target.to_lowercase(),"c:\\program files\\rust-explorer\\rust-explorer.exe");assert_eq!(args,"--recycle-bin");println!("Verified shortcut: {}",path.display());
        }
        if initialized{CoUninitialize();}
    }}
    #[test]
    fn virtual_path_is_recognized(){assert!(is(Path::new(PATH)));assert!(is(Path::new("SHELL:recyclebinfolder")));assert!(!is(Path::new("C:\\RecycleBinFolder")));}
    #[test]
    fn enumerating_system_bin_is_read_only(){unsafe{let initialized=CoInitializeEx(None,COINIT_APARTMENTTHREADED).is_ok();let rows=list(&||false).unwrap();assert!(rows.iter().all(|r|r.recycled.is_some()&&!r.name.is_empty()));if initialized{CoUninitialize();}}}
    #[test]
    #[ignore="creates and restores only a uniquely named temporary test file"]
    fn recycle_and_restore_own_fixture(){unsafe{
        CoInitializeEx(None,COINIT_APARTMENTTHREADED).ok().unwrap();
        // Recover only fixtures made by this test if an earlier development
        // run stopped between recycle and restore. Never touch other entries.
        let temp=env::temp_dir();
        let old=list(&||false).unwrap().into_iter().filter_map(|r|r.recycled).filter(|e|{
            let d=Path::new(&e.original_dir);e.name.starts_with("winrust-bin-fixture-")&&d.parent()==Some(temp.as_path())&&d.file_name().is_some_and(|n|n.to_string_lossy().starts_with("rx-bin-test-"))&&!d.join(&e.name).exists()
        }).collect::<Vec<_>>();
        for entry in old{operate(0,RESTORE,std::slice::from_ref(&entry)).unwrap();let file=Path::new(&entry.original_dir).join(&entry.name);assert_eq!(fs::read_to_string(&file).unwrap(),"restore verification");fs::remove_file(file).unwrap();let _=fs::remove_dir(&entry.original_dir);}
        let dir=env::temp_dir().join(format!("rx-bin-test-{}",std::process::id()));fs::create_dir_all(&dir).unwrap();
        let name=format!("winrust-bin-fixture-{}.txt",std::process::id());let path=dir.join(&name);fs::write(&path,"restore verification").unwrap();
        let item=shell_item(&path.to_string_lossy()).unwrap();let op:IFileOperation=CoCreateInstance(&FileOperation,None,CLSCTX_ALL).unwrap();
        op.SetOperationFlags(FOF_ALLOWUNDO|FOF_NOCONFIRMATION|FOF_NOERRORUI|FOF_SILENT).unwrap();op.DeleteItem(&item,None).unwrap();op.PerformOperations().unwrap();assert!(!path.exists());
        let rows=list(&||false).unwrap();let entries=rows.into_iter().filter_map(|r|r.recycled).filter(|e|e.name==name&&Path::new(&e.original_dir)==dir).collect::<Vec<_>>();
        assert_eq!(entries.len(),1,"must identify ONLY our fixture");operate(0,RESTORE,&entries).unwrap();assert_eq!(fs::read_to_string(&path).unwrap(),"restore verification");fs::remove_dir_all(dir).unwrap();CoUninitialize();
    }}
    #[test]
    #[ignore="permanent deletion and restore collisions use ONLY generated test fixtures"]
    fn own_fixture_collision_and_permanent_delete(){unsafe{
        CoInitializeEx(None,COINIT_APARTMENTTHREADED).ok().unwrap();
        let dir=env::temp_dir().join(format!("rx-bin-collision-{}",std::process::id()));fs::create_dir_all(&dir).unwrap();let name=format!("bin-collision-{}.txt",std::process::id());let path=dir.join(&name);
        let recycle_file=|p:&Path|{let item=shell_item(&p.to_string_lossy()).unwrap();let op:IFileOperation=CoCreateInstance(&FileOperation,None,CLSCTX_ALL).unwrap();op.SetOperationFlags(FOF_ALLOWUNDO|FOF_NOCONFIRMATION|FOF_NOERRORUI|FOF_SILENT).unwrap();op.DeleteItem(&item,None).unwrap();op.PerformOperations().unwrap();};
        let find=||list(&||false).unwrap().into_iter().filter_map(|r|r.recycled).filter(|e|e.name==name&&Path::new(&e.original_dir)==dir).collect::<Vec<_>>();
        fs::write(&path,"restore collision contents").unwrap();recycle_file(&path);let entries=find();assert_eq!(entries.len(),1);fs::write(&path,"keep original destination").unwrap();operate(0,RESTORE,&entries).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(),"keep original destination");assert!(fs::read_dir(&dir).unwrap().flatten().any(|e|fs::read_to_string(e.path()).is_ok_and(|s|s=="restore collision contents")));
        recycle_file(&path);let entries=find();assert_eq!(entries.len(),1);operate(0,DELETE,&entries).unwrap();assert!(find().is_empty());
        fs::remove_dir_all(&dir).unwrap();CoUninitialize();
    }}
}
