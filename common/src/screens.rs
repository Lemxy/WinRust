//! Physical desktop coordinates, including monitors above/left of the primary.
use windows_sys::Win32::{Foundation::*,Graphics::Gdi::*,UI::HiDpi::*};
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct Bounds{pub left:i32,pub top:i32,pub right:i32,pub bottom:i32}
impl Bounds{
    pub fn width(self)->i32{(self.right-self.left).max(1)}
    pub fn height(self)->i32{(self.bottom-self.top).max(1)}
    pub fn rect(self)->RECT{RECT{left:self.left,top:self.top,right:self.right,bottom:self.bottom}}
    pub fn contains(self,x:i32,y:i32)->bool{x>=self.left&&x<self.right&&y>=self.top&&y<self.bottom}
    pub fn fit(self,x:i32,y:i32,w:i32,h:i32)->(i32,i32){(x.clamp(self.left,(self.right-w).max(self.left)),y.clamp(self.top,(self.bottom-h).max(self.top)))}
}
impl From<RECT> for Bounds{fn from(r:RECT)->Self{Self{left:r.left,top:r.top,right:r.right,bottom:r.bottom}}}
#[derive(Clone,Copy,Debug)]
pub struct Screen{pub bounds:Bounds,pub work:Bounds,pub dpi:u32,pub primary:bool}
unsafe fn info(m:HMONITOR)->Screen{unsafe{
    let mut i:MONITORINFO=std::mem::zeroed();i.cbSize=std::mem::size_of::<MONITORINFO>()as u32;
    GetMonitorInfoW(m,&mut i);
    let(mut x,mut y)=(96,96);let _=GetDpiForMonitor(m,MDT_EFFECTIVE_DPI,&mut x,&mut y);
    Screen{bounds:i.rcMonitor.into(),work:i.rcWork.into(),dpi:x.max(96),primary:i.dwFlags&1!=0}
}}
pub fn all()->Vec<Screen>{
    unsafe extern "system" fn collect(m:HMONITOR,_:HDC,_:*mut RECT,data:LPARAM)->i32{unsafe{(*(data as *mut Vec<Screen>)).push(info(m));}1}
    let mut screens=Vec::new();unsafe{EnumDisplayMonitors(std::ptr::null_mut(),std::ptr::null(),Some(collect),&mut screens as *mut _ as isize);}screens
}
pub fn for_window(h:HWND)->Screen{unsafe{info(MonitorFromWindow(h,MONITOR_DEFAULTTONEAREST))}}
pub fn primary()->Screen{unsafe{info(MonitorFromPoint(POINT{x:0,y:0},MONITOR_DEFAULTTOPRIMARY))}}
pub fn union(screens:&[Screen])->Bounds{
    screens.iter().map(|s|s.bounds).reduce(|a,b|Bounds{left:a.left.min(b.left),top:a.top.min(b.top),right:a.right.max(b.right),bottom:a.bottom.max(b.bottom)}).unwrap_or(Bounds{left:0,top:0,right:1920,bottom:1080})
}
pub fn virtual_bounds()->Bounds{union(&all())}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn negative_origins_union_and_popup_clamp(){
        let b=Bounds{left:-2560,top:-200,right:0,bottom:1240};let a=Bounds{left:0,top:0,right:1920,bottom:1080};
        let screens=[Screen{bounds:a,work:a,dpi:96,primary:true},Screen{bounds:b,work:b,dpi:144,primary:false}];
        assert_eq!(union(&screens),Bounds{left:-2560,top:-200,right:1920,bottom:1240});
        assert_eq!(b.fit(-2700,-300,300,200),(-2560,-200));assert_eq!(b.fit(50,1300,300,200),(-300,1040));
        assert!(!b.contains(0,0));assert!(b.contains(-1,0));
    }
    #[test]fn connected_monitors_have_valid_physical_bounds(){
        let screens=all();println!("Connected monitors: {screens:?}");assert!(!screens.is_empty());
        let virtual_screen=union(&screens);for s in screens{assert!(s.bounds.width()>0&&s.bounds.height()>0&&s.dpi>=96);assert!(virtual_screen.contains(s.bounds.left,s.bounds.top));assert!(virtual_screen.contains(s.bounds.right-1,s.bounds.bottom-1));}
    }
}
