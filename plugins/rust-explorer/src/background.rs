//! Bounded workers and coalesced requests. No HWND-owned state leaves the UI thread.
use super::*;
use std::sync::{Arc, Condvar, Mutex, atomic::{AtomicU64, Ordering}, mpsc};

pub const READY: u32 = WM_APP + 20;
pub const CHANGED: u32 = WM_APP + 21;
pub const REFRESH_TIMER: usize = 21;
pub const ICON_READY:u32=WM_APP+23;
pub struct Icons{tx:mpsc::SyncSender<(u32,usize,PathBuf,bool)>,rx:mpsc::Receiver<(u32,usize,i32)>}
impl Icons{
    pub fn new(window:isize)->Self{
        let(tx,requests)=mpsc::sync_channel::<(u32,usize,PathBuf,bool)>(256);let(results,rx)=mpsc::channel();
        std::thread::spawn(move||unsafe{
            let initialized=windows::Win32::System::Com::CoInitializeEx(None,windows::Win32::System::Com::COINIT_APARTMENTTHREADED).is_ok();
            for(era,row,path,dir)in requests{
                if era!=GEN.load(Relaxed){continue;}
                let icon=icon_of(&path,dir);
                if era!=GEN.load(Relaxed){continue;}
                if results.send((era,row,icon)).is_err(){break;}
                PostMessageW(window as HWND,ICON_READY,0,0);
            }
            if initialized{windows::Win32::System::Com::CoUninitialize();}
        });Self{tx,rx}
    }
    pub fn request(&self,row:usize,path:PathBuf,dir:bool)->bool{self.tx.try_send((GEN.load(Relaxed),row,path,dir)).is_ok()}
    pub fn take(&self)->Vec<(u32,usize,i32)>{self.rx.try_iter().collect()}
}

pub struct Request {
    pub id: u64, pub dir: PathBuf, pub query: Option<String>, pub desktop: bool,
    pub extras: Vec<PathBuf>, pub hidden: bool, pub col: usize, pub asc: bool,
    pub history: bool, pub refresh: bool, pub pick: Option<String>,
}
pub struct ResultSet { pub request: Request, pub rows: Result<Vec<Row>, String>, pub title: Option<String> }
struct Queue { pending: Option<Request>, stopped: bool }
pub struct Worker {
    queue: Arc<(Mutex<Queue>, Condvar)>, generation: Arc<AtomicU64>,
    rx: mpsc::Receiver<ResultSet>, pub busy: bool,
}
impl Worker {
    pub fn new(window: isize) -> Self {
        let queue = Arc::new((Mutex::new(Queue { pending: None, stopped: false }), Condvar::new()));
        let generation = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        // A slow filesystem call may not be interruptible. Keep the UI free,
        // allow another request to proceed, and never spawn an unbounded pool.
        for _ in 0..2 {
            let (queue, generation, tx) = (queue.clone(), generation.clone(), tx.clone());
            std::thread::spawn(move || {
                unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED); }
                loop {
                    let mut request = {
                        let (lock, wake) = &*queue;
                        let mut q = lock.lock().unwrap();
                        while q.pending.is_none() && !q.stopped { q = wake.wait(q).unwrap(); }
                        if q.stopped { break; }
                        q.pending.take().unwrap()
                    };
                    let cancelled = || generation.load(Ordering::Acquire) != request.id;
                    if cancelled() { continue; }
                    if request.query.is_none()&&!request.desktop&&!recycle::is(&request.dir)&&zipx::split_zip(&request.dir).is_none()&&request.dir.is_file(){
                        request.pick=request.dir.file_name().map(|n|n.to_string_lossy().into_owned());
                        request.dir=request.dir.parent().unwrap_or(&request.dir).to_path_buf();
                    }
                    let (rows, title) = if let Some(query) = &request.query {
                        search_rows(&request.dir, query, &cancelled)
                    } else {
                        (rows_for(&request.dir, request.desktop, &request.extras, request.hidden, request.col, request.asc, &cancelled).ok_or_else(|| "Не удалось открыть папку".into()), None)
                    };
                    if cancelled() { continue; }
                    if tx.send(ResultSet { request, rows, title }).is_err() { break; }
                    // Payload stays owned by the channel even if posting fails
                    // or the window closes. No raw Box can leak into the queue.
                    unsafe { PostMessageW(window as HWND, READY, 0, 0); }
                }
                unsafe { windows::Win32::System::Com::CoUninitialize(); }
            });
        }
        Self { queue, generation, rx, busy: false }
    }
    pub fn submit(&mut self, mut request: Request) {
        request.id = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.busy = true;
        self.queue.0.lock().unwrap().pending = Some(request);
        self.queue.1.notify_one();
    }
    pub fn take(&mut self) -> Option<ResultSet> {
        let mut latest = None;
        while let Ok(r) = self.rx.try_recv() {
            if r.request.id == self.generation.load(Ordering::Acquire) { latest = Some(r); }
        }
        if latest.is_some() { self.busy = false; }
        latest
    }
    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.queue.0.lock().unwrap().pending = None;
        self.busy = false;
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.queue.0.lock().unwrap().stopped = true;
        self.queue.1.notify_all();
    }
}

struct Event(usize);
impl Drop for Event { fn drop(&mut self) { unsafe { CloseHandle(self.0 as _); } } }
pub struct Watcher { stop: Arc<Event> }
impl Drop for Watcher { fn drop(&mut self) { unsafe { windows_sys::Win32::System::Threading::SetEvent(self.stop.0 as _); } } }
impl Watcher {
    pub fn start(window: isize, dirs: Vec<PathBuf>, generation: usize, recursive:bool) -> Self {
        use windows_sys::Win32::{Storage::FileSystem::*, System::Threading::*};
        let stop = Arc::new(Event(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) } as usize));
        let keep = stop.clone();
        std::thread::spawn(move || unsafe {
            let mut changes = Vec::new();
            for dir in dirs {
                if WaitForSingleObject(keep.0 as _, 0) == WAIT_OBJECT_0 { break; }
                let h = FindFirstChangeNotificationW(w(&dir.to_string_lossy()).as_ptr(), recursive as i32,
                    FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_ATTRIBUTES);
                if h != INVALID_HANDLE_VALUE && !h.is_null() { changes.push(h); }
            }
            if !changes.is_empty() {
                // Reconcile once after watches are armed: no file change can
                // be lost in the gap between initial listing and registration.
                PostMessageW(window as HWND, CHANGED, generation, 0);
                let handles: Vec<HANDLE> = std::iter::once(keep.0 as HANDLE).chain(changes.iter().copied()).collect();
                loop {
                    let result = WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE);
                    if result == WAIT_OBJECT_0 || result == WAIT_FAILED { break; }
                    let i = result.wrapping_sub(WAIT_OBJECT_0 + 1) as usize;
                    if i >= changes.len() { break; }
                    PostMessageW(window as HWND, CHANGED, generation, 0);
                    if FindNextChangeNotification(changes[i]) == 0 { break; }
                }
            }
            for h in changes { FindCloseChangeNotification(h); }
        });
        Self { stop }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(dir: PathBuf) -> Request { Request { id:0,dir,query:None,desktop:false,extras:vec![],hidden:true,col:0,asc:true,history:false,refresh:false,pick:None } }
    #[test]
    fn latest_request_wins_and_cancel_discards_ready_results() {
        let dir=env::temp_dir().join(format!("rx-worker-{}",std::process::id())); fs::create_dir_all(&dir).unwrap(); fs::write(dir.join("keep.txt"),"x").unwrap();
        let mut worker=Worker::new(0);
        worker.submit(request(dir.join("missing"))); worker.submit(request(dir.clone()));
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
        let result=loop { if let Some(r)=worker.take(){break r;} assert!(std::time::Instant::now()<deadline); std::thread::sleep(std::time::Duration::from_millis(10)); };
        assert_eq!(result.request.dir,dir); assert_eq!(result.rows.unwrap()[0].name,"keep.txt");
        worker.submit(request(dir.clone())); worker.cancel(); std::thread::sleep(std::time::Duration::from_millis(80)); assert!(worker.take().is_none());
        drop(worker); fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn directory_watcher_signals_real_changes_and_stops() {
        unsafe {
            let dir=env::temp_dir().join(format!("rx-watch-{}",std::process::id())); fs::create_dir_all(&dir).unwrap();
            let h=CreateWindowExW(0,w("STATIC").as_ptr(),ptr::null(),0,0,0,10,10,ptr::null_mut(),ptr::null_mut(),GetModuleHandleW(ptr::null()),ptr::null()); assert!(!h.is_null());
            let watcher=Watcher::start(h as isize,vec![dir.clone()],42,false);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
            let mut msg:MSG=std::mem::zeroed();
            // First notification only acknowledges registration; it must not
            // accidentally satisfy the real filesystem-change assertion.
            while PeekMessageW(&mut msg,h,CHANGED,CHANGED,PM_REMOVE)==0{assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(10));}
            loop { fs::write(dir.join("event.txt"),"change").unwrap(); std::thread::sleep(std::time::Duration::from_millis(40)); if PeekMessageW(&mut msg,h,CHANGED,CHANGED,PM_REMOVE)!=0 {break;} assert!(std::time::Instant::now()<deadline); }
            assert_eq!(msg.wParam,42); drop(watcher); DestroyWindow(h); fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn stalled_indexer_has_a_deadline(){unsafe{
        use windows_sys::Win32::{System::Pipes::*,Storage::FileSystem::PIPE_ACCESS_DUPLEX};
        let pipe=CreateNamedPipeW(w(r"\\.\pipe\rs-search-Ω").as_ptr(),PIPE_ACCESS_DUPLEX,PIPE_TYPE_BYTE|PIPE_WAIT,1,4096,4096,0,ptr::null());assert_ne!(pipe,INVALID_HANDLE_VALUE);
        let raw=pipe as usize;let server=std::thread::spawn(move||{let pipe=raw as HANDLE;ConnectNamedPipe(pipe,ptr::null_mut());std::thread::sleep(std::time::Duration::from_millis(250));DisconnectNamedPipe(pipe);CloseHandle(pipe);});
        let start=std::time::Instant::now();assert!(ask('Ω',"test",&||false,start+std::time::Duration::from_millis(80)).is_none());assert!(start.elapsed()<std::time::Duration::from_secs(1));server.join().unwrap();
    }}
}
