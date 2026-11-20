"""Independent complete Fraction sizing oracle; no production helper imports."""
from fractions import Fraction as F
from pathlib import Path
import json
MAX=(1<<96)-1

def native(v):
    for scale in range(29):
        c=v*10**scale
        if c.denominator==1 and 0<=c.numerator<=MAX:
            n=str(c.numerator).rjust(scale+1,'0')
            return n if scale==0 else n[:-scale]+'.'+n[-scale:]
    raise ValueError('Unrepresentable')
def floor(v,p):return F(v*10**p//1,10**p)
def grid(v,step,up=False):
    q=v/step
    n=q.numerator//q.denominator
    return (n+int(up and q.denominator!=1))*step
# Separate labeled analyzer-support observations. No percentage stop creation.
cases=[
('initial','100','50','10000','50','0.1','1','0','0','0'),
('one-reduction','100','50','2000','2000','0.1','1','0','0','0'),
('two-reductions','100','60','2000','2000','0.1','1','0','0','0'),
('three-reductions','100','70','1800','2000','0.1','1','0','0','0'),
('fourth-forbidden','100','80','2000','2000','0.1','1','0','0','0'),
('risk-floor12','3','0.3','100','100','0.0000000000001','0.0000000000000000000000000001','0','0','0'),
('physical-floor8','3','0.3','1000000','1','0.0000000001','0.0000000000000000000000000001','0','0','0'),
('reserve-margin','100','50','1000000','50','0.0001','1','0','0','200'),
('gross-cash','100','50','100000','105','0.1','1','10','0','0'),
('named-chart','73284.01','65955.60','1805','1805','0.00001','5','10','10','200'),
('S0-unrepresentable','0.0000000000000000000000000002','0.0000000000000000000000000001','2000','2000',str(MAX),'1','0','0','0'),
('R0-unrepresentable','0.2','0.1','100000','1','0.1',str(MAX),'0','0','0'),
('final-unrepresentable','1.01','0.5','1000000','0.00000001515','0.0000000000000000000000000001','0.000000000001','10','0','0'),
('large-trigger','9000000000000000000000000000','7000000000000000000000000000',str(MAX),str(MAX),'0.1','1','0','100','0'),
('tiny-quote-cancellation','0.0000000000000000000000000100','0.0000000000000000000000000050','1000','1000','1','0.0000000000000000000000000001','0','0','0'),
]
rows=[]
for name,p,c,b,cash,step,minimum,fee,gap,margin in cases:
    row=dict(name=name,price=p,chart=c,basis=b,free=cash,step=step,minimum=minimum,fee_bps=fee,gap=gap,margin=margin)
    P,C,B,CASH,STEP,MIN,FEE,GAP,MARGIN=map(F,(p,c,b,cash,step,minimum,fee,gap,margin))
    for v in (P,C,B,CASH,STEP,MIN,FEE,GAP,MARGIN):native(v)
    native(P-C) # These fixtures supply an exactly representable span.
    fee=FEE/10000;t=1-fee;u=1-MARGIN/10000
    af=C*(1-GAP/10000);loss=(P-af)+fee*(P+af)
    cap=B/100;physical=min(B,CASH)
    try:
        native(af);native(loss)
        s0=grid(min(floor(cap/loss,12),floor(physical/P,8)),STEP)
        r0=grid(MIN/(af*u),STEP,True)
        row['s0_native']=True;row['r0_native']=True
        for key,value in [('s0_native',s0),('r0_native',r0)]:
            try:native(value)
            except ValueError:row[key]=False
        for k in range(4):
            s=s0-k*STEP;row['attempts']=k+1
            if s<=0:row['reason']='Quantity';break
            try:
                native(s)
                if s<STEP:row['reason']='Quantity';break
                if s>MAX:row['reason']='Quantity';continue
                q=grid(s/t,STEP,True);native(q)
                if q<STEP or q>MAX:row['reason']='Quantity';continue
                rh=min(s,r0);native(rh)
                charge=s*loss+(q-s)*P+rh*af*t;gross=q*P
                if charge>cap:row['reason']='ChargeCap';continue
                if gross>physical:row['reason']='PhysicalCap';continue
                if gross*u<MIN:row['reason']='EntryMinimum';continue
                if s*af*u<MIN:row['reason']='ProtectionMinimum';continue
                row['amounts']=[native(v) for v in (s,q,af,loss,rh,charge,gross)]
                row.pop('reason',None);break
            except ValueError:row['reason']='Unrepresentable'
    except ValueError:row['error']='Unrepresentable'
    rows.append(row)
Path(__file__).with_suffix('.json').write_text(json.dumps(rows,indent=2)+'\n')
print('\n'.join(f"{r['name']}: {r.get('reason',r.get('error','Accepted'))} attempts={r.get('attempts')}" for r in rows))
