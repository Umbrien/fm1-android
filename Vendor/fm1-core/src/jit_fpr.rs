//! Block-local floating-point register allocation. Guest registers retain raw
//! 32-bit values; integer uses transfer bits, without converting or quieting NaNs.
use super::{Fast, Gen, Reg};

pub(super) fn allocate(ops:&[crate::BOp])->Vec<(u8,Reg)> {
    let mut float=[0u32;16];let mut integer=[0u32;16];
    for op in ops {
        if let Fast::FOp{d,a,b,op}=op.fast {
            let (d,a,b)=((d&15)as usize,(a&15)as usize,(b&15)as usize);
            match op {
                0x1f|0x5f=>{float[a]+=1;integer[d]+=1;}
                0x8f|0x9f=>{integer[a]+=1;float[d]+=1;}
                7|8=>{float[a]+=1;float[b]+=1;float[d]+=2;}
                _=>{float[a]+=1;float[b]+=1;float[d]+=1;}
            }
        }else if let Some((reads,writes))=op.fast.regs_rw(){
            for r in 0..16{integer[r]+=u32::from(reads&(1<<r)!=0)+u32::from(writes&(1<<r)!=0);}
        }else{
            // Opaque helpers/inline raw operations can involve any guest register.
            for n in &mut integer{*n+=2;}
        }
    }
    let mut order:Vec<_>=(0..16).filter(|&r|float[r]>=4&&float[r]>integer[r]).collect();
    order.sort_by_key(|&r|std::cmp::Reverse(float[r]-integer[r]));
    order.into_iter().enumerate().map(|(i,r)|(r as u8,16+i as Reg)).collect()
}

impl Gen<'_> {
    pub(super) fn fp_cached(&self,r:u8)->Option<Reg>{
        if !self.cache_active{return None;}
        self.fp_cached_regs.iter().find(|&&(guest,_)|guest==r&15).map(|&(_,host)|host)
    }
    fn fp_operand(&mut self,r:u8,scratch:Reg)->Reg{
        if let Some(host)=self.fp_cached(r){host}else{
            self.ld_reg(0,r);self.a.fmov_s_w(scratch,0);scratch
        }
    }
    fn fp_result(&mut self,d:u8,result:Reg){
        if self.fp_cached(d)!=Some(result){self.a.fmov_w_s(0,result);self.st_reg(0,d);}
    }
}

/// Leave the original lowering intact when no allocated FPR participates, and
/// for MACs whose nonfinite fallback and separately rounded product stay shared.
pub(super) fn emit(g:&mut Gen<'_>,fast:Fast)->bool{
    let Fast::FOp{d,a,b,op}=fast else{return false;};
    if matches!(op,7|8){return false;}
    let relevant=g.fp_cached(d).is_some()||g.fp_cached(a).is_some()
        ||(!matches!(op,0x1f|0x5f|0x8f|0x9f)&&g.fp_cached(b).is_some());
    if !relevant{return false;}
    match op {
        0x1f|0x5f=>{
            let input=g.fp_operand(a,0);
            if op==0x1f{g.a.fcvtzs(0,input);}else{g.a.fcvtzu(0,input);}
            g.st_reg(0,d);
        }
        0x8f|0x9f=>{
            g.ld_reg(0,a);let dest=g.fp_cached(d).unwrap_or(0);
            if op==0x8f{g.a.scvtf(dest,0);}else{g.a.ucvtf(dest,0);}
            g.fp_result(d,dest);
        }
        _=>{
            // Read all source bits before writing d, including arbitrary aliases.
            let left=g.fp_operand(a,0);let right=g.fp_operand(b,1);
            let dest=g.fp_cached(d).unwrap_or(0);
            let opcode=match op{0=>0x1e202800,1=>0x1e203800,2=>0x1e200800,
                3=>0x1e201800,5=>0x1e207800,_=>0x1e206800};
            g.a.fop(opcode,dest,left,right);g.fp_result(d,dest);
        }
    }
    true
}
