"""Independent Fraction geometry goldens; no Rust/shared arithmetic imports."""
from fractions import Fraction as F
from pathlib import Path
import json
MAX=(1<<96)-1

def native(v):
    for s in range(29):
        c=v*10**s
        if c.denominator==1 and 0<=c.numerator<=MAX:
            n=str(c.numerator).rjust(s+1,'0')
            return n if s==0 else n[:-s]+'.'+n[-s:]
    raise ValueError('Unrepresentable')
# Actual labeled analyzer-support observations; never derived as a percentage.
cases=[('100','50',None,'0','0.1','span'),('100','50','49.73','100','0.1','span'),('100','50','55','100','0.1','legacy'),('100','99.5',None,'100','0.1','span'),('73284.01','65955.60',None,'0','0.01','span'),('900000000000000000000000000','700000000000000000000000000',None,'100','1','span'),(str(MAX),'0.1',None,'0','0.1','span'),(str(MAX),'0.1',None,'0','0.1','legacy'),('1','0.5000000000000000000000000001',None,'1','0.0000000000000000000000000001','span'),('0.0000000000000000000000000100','0.0000000000000000000000000050',None,'0','0.0000000000000000000000000001','span')]
rows=[]
for p,c,g,b,t,policy in cases:
    row=dict(price=p,chart=c,guard=g,buffer=b,tick=t,policy=policy)
    P,C,G,B,T=F(p),F(c),F(g) if g else None,F(b),F(t)
    A=min(C,G) if G else C
    try:
        dc=P-A
        if policy=='span':native(dc)
        buffer=A*B/10000
        if policy=='span':buffer=min(buffer,dc/4)
        native(buffer)
        trigger=A-buffer
        if policy=='span':trigger=(trigger//T)*T
        span=P-trigger
        values=dict(basis=native(A),effective_buffer=native(buffer),trigger=native(trigger),guard_bound=A<C,cap_basis=native(dc) if policy=='span' else None,span=native(span) if policy=='span' else None)
        row['expected']=values
    except ValueError as error:row['error']=str(error)
    rows.append(row)
Path(__file__).with_suffix('.json').write_text(json.dumps(rows,indent=2)+'\n')
