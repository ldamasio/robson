# Independent exact oracle; no runtime helper or Decimal rounding.
from fractions import Fraction as F
from pathlib import Path
import json,hashlib
M='79228162514264337593543950335'
cases=[('add',M,'0'),('add',M,'1'),('sub',M,'1'),('mul',M,'0.0000000000000000000000000001'),('mul','900000000000000000000000000','100'),('div','1','3'),('div','1','10000000000000000000000000000'),('div','0.0000000000000000000000000001','10'),('ceil','0.1000000000000000000000000001','0.1'),('ceil','0.3','0.1'),('floor12','0.1234567890129999999999999999','0'),('floor8','0.1234567899999999999999999999','0'),('add','0.0000000000000000000000000001','0.0000000000000000000000000002'),('sub','0.01','0.02'),('div','1','0'),('floor','0.29','0.07'),('ceil',M,'2')]
def export(v):
 if v<0:return 'Negative'
 for scale in range(29):
  q,r=divmod(v.numerator*10**scale,v.denominator)
  if q>2**96-1:return 'Unrepresentable'
  if r==0:
   if scale==0:return str(q)
   digits=str(q).rjust(scale+1,'0');return digits[:-scale]+'.'+digits[-scale:]
 return 'Unrepresentable'
rows=[]
for op,a,b in cases:
 x,y=F(a),F(b)
 try:
  if op=='add':z=x+y
  elif op=='sub':z=x-y
  elif op=='mul':z=x*y
  elif op=='div':z=x/y
  elif op=='floor':z=(x//y)*y
  elif op=='ceil':z=(-(-x//y))*y
  else:
   n=12 if op=='floor12' else 8;z=(x*10**n//1)/F(10**n)
  expected=export(z)
 except ZeroDivisionError:expected='ZeroDenominator'
 rows.append(dict(op=op,a=a,b=b,expected=expected))
p=Path(__file__).with_name('arithmetic.json');p.write_text(json.dumps(rows,indent=2)+'\n');print(hashlib.sha256(p.read_bytes()).hexdigest())
