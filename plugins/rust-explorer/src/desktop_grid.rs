//! An icon's native position is not necessarily the top-left of its label.
//! Each monitor gets its own inset grid; the complete native item must fit.
use std::collections::HashSet;
use winrust_common::screens::{Bounds, Screen};

#[derive(Clone, Copy, Debug)]
pub struct Footprint { pub left:i32, pub top:i32, pub right:i32, pub bottom:i32 }
impl Footprint {
    pub fn at(self, p:(i32,i32))->Bounds { Bounds{left:p.0+self.left,top:p.1+self.top,right:p.0+self.right,bottom:p.1+self.bottom} }
}
pub fn contains_rect(outer:Bounds, inner:Bounds)->bool {
    inner.left>=outer.left && inner.top>=outer.top && inner.right<=outer.right && inner.bottom<=outer.bottom
}
struct Slot { pos:(i32,i32), monitor:usize }
pub struct Grid {
    slots:Vec<Slot>, screens:Vec<Screen>, origin:(i32,i32), icon_center:(i32,i32),
}
impl Grid {
    pub fn new(screens:&[Screen],canvas:Bounds,spacing:(i32,i32),footprint:Footprint,icon:(i32,i32),padding:i32)->Self {
        let mut order=(0..screens.len()).collect::<Vec<_>>();
        order.sort_by_key(|&i|(!screens[i].primary,screens[i].bounds.top,screens[i].bounds.left));
        let mut slots=vec![];
        // Keep the control's Windows icon spacing. Native hit/selection bounds
        // may overlap neighbouring cells; they constrain the monitor edges,
        // rather than increasing the distance between all desktop icons.
        let sx=spacing.0.max(1);
        let sy=spacing.1.max(1);
        for monitor in order {
            let work=screens[monitor].work;
            let first=(work.left+padding-footprint.left,work.top+padding-footprint.top);
            let last=(work.right-padding-footprint.right,work.bottom-padding-footprint.bottom);
            if first.0>last.0 || first.1>last.1 {continue;}
            for column in 0..=(last.0-first.0)/sx { for row in 0..=(last.1-first.1)/sy {
                let global=(first.0+column*sx,first.1+row*sy);
                if contains_rect(canvas,footprint.at(global)) {
                    slots.push(Slot{pos:(global.0-canvas.left,global.1-canvas.top),monitor});
                }
            }}
        }
        Self{slots,screens:screens.to_vec(),origin:(canvas.left,canvas.top),icon_center:(icon.0/2,icon.1/2)}
    }
    pub fn at(&self,index:usize)->(i32,i32){self.slots[index].pos}
    fn owner(&self,p:(i32,i32))->Option<usize>{
        let center=(p.0+self.origin.0+self.icon_center.0,p.1+self.origin.1+self.icon_center.1);
        self.screens.iter().position(|s|s.bounds.contains(center.0,center.1))
    }
    fn distance(a:(i32,i32),b:(i32,i32))->i64 {let x=i64::from(a.0)-i64::from(b.0);let y=i64::from(a.1)-i64::from(b.1);x*x+y*y}
    pub fn cell(&self,p:(i32,i32))->Option<usize>{
        let monitor=self.owner(p)?;
        self.slots.iter().enumerate().filter(|(_,s)|s.monitor==monitor).min_by_key(|(_,s)|Self::distance(s.pos,p)).map(|(i,_)|i)
    }
    /// Explicit drags stay on their destination monitor while it has room.
    /// Automatic placement starts down the primary monitor's columns.
    pub fn free(&self,taken:&HashSet<usize>,want:Option<(i32,i32)>)->Option<usize>{
        let owner=want.and_then(|p|self.owner(p));
        self.slots.iter().enumerate().filter(|(i,_)|!taken.contains(i)).min_by_key(|(i,s)|{
            if let Some(p)=want {(owner.is_some_and(|m|m!=s.monitor),Self::distance(s.pos,p),*i)} else {(false,0,*i)}
        }).map(|(i,_)|i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screen(left:i32,top:i32,right:i32,bottom:i32,primary:bool)->Screen{let b=Bounds{left,top,right,bottom};Screen{bounds:b,work:b,dpi:96,primary}}
    fn footprint()->Footprint{Footprint{left:-28,top:0,right:60,bottom:80}}
    #[test]
    fn labels_fit_independent_grids_on_both_sides_of_the_seam(){
        let screens=[screen(-1920,0,0,1080,false),screen(0,0,1920,1080,true)];let canvas=winrust_common::screens::union(&screens);
        let g=Grid::new(&screens,canvas,(88,96),footprint(),(32,32),8);
        assert_eq!(g.at(0),(1956,8));
        for s in &g.slots {let global=(s.pos.0+canvas.left,s.pos.1+canvas.top);let bounds=footprint().at(global);assert!(contains_rect(screens[s.monitor].work,bounds));assert!(bounds.left>=screens[s.monitor].work.left+8);assert!(bounds.right<=screens[s.monitor].work.right-8);}
        let left=g.cell((0,0)).unwrap();assert_eq!(g.at(left),(36,8));
        assert_eq!(g.at(g.cell((1920,0)).unwrap()),(1956,8));
    }
    #[test]
    fn primary_positions_do_not_depend_on_a_left_or_upper_monitor(){
        let main=screen(0,0,1920,1080,true);let single=Grid::new(&[main],main.bounds,(88,96),footprint(),(32,32),8);
        let screens=[screen(-2560,-200,0,1240,false),main];let canvas=winrust_common::screens::union(&screens);let multi=Grid::new(&screens,canvas,(88,96),footprint(),(32,32),8);
        for(i,s)in single.slots.iter().enumerate(){assert_eq!(multi.slots[i].pos,(s.pos.0-canvas.left,s.pos.1-canvas.top));}
    }
    #[test]
    fn drag_targets_clamp_inside_one_monitor_and_never_create_invalid_cells(){
        let screens=[screen(-1280,-300,0,720,false),screen(0,0,1920,1080,true)];let canvas=winrust_common::screens::union(&screens);let g=Grid::new(&screens,canvas,(88,96),footprint(),(32,32),8);
        let mut taken=HashSet::new();
        for p in [(1280,300),(1270,300),(1285,300),(-500,-500),(9000,9000)]{let i=g.free(&taken,Some(p)).unwrap();assert!(taken.insert(i));let s=&g.slots[i];assert!(contains_rect(screens[s.monitor].work,footprint().at((s.pos.0+canvas.left,s.pos.1+canvas.top))));}
        let all=(0..g.slots.len()).collect();assert!(g.free(&all,None).is_none());
    }
}
