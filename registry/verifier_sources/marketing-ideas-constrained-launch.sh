#!/usr/bin/env bash
python3 - <<'PYEOF'
from pathlib import Path
import re, sys
path=Path('tasks/marketing-ideas/marketing-plan.md'); raw=path.read_text(errors='ignore') if path.is_file() else ''; low=raw.lower(); errors=[]
fields=[('channel',r'\bchannel\b'),('hook/message',r'\b(?:hook|message)\b'),('rationale',r'\b(?:why|rationale)\b'),('cash cost',r'\b(?:cash\s+)?cost\b'),('founder time',r'\bfounder\s+time\b'),('success metric',r'\b(?:success\s+metric|metric|kpi)\b'),('first test',r'\b(?:first|smallest|initial)\s+test\b')]
table_lines=[line for line in raw.splitlines() if line.strip().startswith('|') and line.strip().endswith('|')]
table_ideas=[]
for i,line in enumerate(table_lines):
    cells=[c.strip().lower() for c in line.strip().strip('|').split('|')]
    if all(any(re.search(pat,c) for c in cells) for _,pat in fields):
        for row in table_lines[i+2:]:
            values=[c.strip().strip('*_ ') for c in row.strip().strip('|').split('|')]
            if len(values)!=len(cells) or any(not v for v in values): break
            table_ideas.append(values)
        break
heads=list(re.finditer(r'^##\s+(.+)$',raw,re.I|re.M))
reserved=re.compile(r'30[- ]day|sequence|budget|summary|overview|five launch ideas|launch ideas|notes?|constraints?|method',re.I)
numbered=list(re.finditer(r'^#{2,3}\s+(?:(?:idea|play|experiment)\s*)?[1-5][.):-]?\s+(.+)$',raw,re.I|re.M))
ideas=numbered if numbered else [h for h in heads if not reserved.search(h.group(1))]
if table_ideas:
    if len(table_ideas)!=5: errors.append(f'plan table must contain exactly five ideas; found {len(table_ideas)}')
else:
    if len(ideas)!=5: errors.append(f'plan must contain exactly five idea sections; found {len(ideas)}')
    for i,h in enumerate(ideas):
        end=ideas[i+1].start() if i+1<len(ideas) else (next((x.start() for x in heads if x.start()>h.start() and reserved.search(x.group(1))),len(raw)))
        section=raw[h.start():end].lower()
        for label,pat in fields:
            if not re.search(pat,section): errors.append(f'idea {i+1} misses {label}')
if not re.search(r'30[- ]day',low) or not re.search(r'\b(?:days?|weeks?)\s*[1-4]\b',low): errors.append('missing concrete 30-day sequence')
budget_section=re.split(r'^#{2,3}\s+.*budget.*$',raw,flags=re.I|re.M)
budget=budget_section[-1] if len(budget_section)>1 else ''
budget_lines=[]; in_table=False
for line in budget.splitlines():
    if line.strip().startswith('|') and line.strip().endswith('|'): budget_lines.append(line); in_table=True
    elif in_table and line.strip(): break
budget_table='\n'.join(budget_lines)
declared=re.search(r'\btotal\b[^\n$]*\$\s*([0-9][0-9,]*(?:\.\d{1,2})?)',budget_table,re.I)
if not budget or not declared: errors.append('budget section needs an explicit total')
else:
    total=float(declared.group(1).replace(',',''))
    line_amounts=[float(x.replace(',','')) for line in budget_lines if 'total' not in line.lower() for x in re.findall(r'\$\s*([0-9][0-9,]*(?:\.\d{1,2})?)',line)]
    if total>1000: errors.append(f'budget exceeds $1,000: ${total:g}')
    if line_amounts and abs(sum(line_amounts)-total)>0.01: errors.append(f'budget line items sum to ${sum(line_amounts):g}, not ${total:g}')
if not re.search(r'\b12\b[^.\n]{0,30}\bhours?\b|\bhours?\b[^.\n]{0,30}\b12\b',low): errors.append('plan does not account for the 12-hour founder limit')
if not re.search(r'\bfree[- ]trial\b',low): errors.append('plan does not account for the free trial')
claims=low
for term in ['existing audience','our customers say','guaranteed','industry-leading']:
    claims=re.sub(rf'\b(?:no|without|avoid|never|not)\b[^.\n]{{0,100}}\b{re.escape(term)}\b','',claims)
    if re.search(rf'\b{re.escape(term)}\b',claims): errors.append(f'unsupported claim detected: {term}')
for e in errors: print('FAIL',e)
sys.exit(0 if not errors else 1)
PYEOF
