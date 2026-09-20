#!/usr/bin/env python3
import ast, json, operator, sys
request = json.load(sys.stdin)
state = request.get('context', {}).get('engine', {}).get('state') or {}
expression = state.get('input', '').strip()
if not expression:
    json.dump({'version': 1, 'items': []}, sys.stdout); sys.stdout.write('\n'); raise SystemExit
allowed = {ast.Add: operator.add, ast.Sub: operator.sub, ast.Mult: operator.mul, ast.Div: operator.truediv}
def ev(n):
    if isinstance(n, ast.Expression): return ev(n.body)
    if isinstance(n, ast.Constant) and isinstance(n.value, (int, float)): return n.value
    if isinstance(n, ast.BinOp) and type(n.op) in allowed: return allowed[type(n.op)](ev(n.left), ev(n.right))
    if isinstance(n, ast.UnaryOp) and isinstance(n.op, (ast.UAdd, ast.USub)): return ev(n.operand) * (1 if isinstance(n.op, ast.UAdd) else -1)
    raise ValueError()
try:
    result = ev(ast.parse(expression, mode='eval'))
    text = f'{result:g}'
except Exception:
    json.dump({'version': 1, 'items': []}, sys.stdout, separators=(',', ':'))
    sys.stdout.write('\n')
    raise SystemExit
item = {'display': {'rows': [{'cells': [{'text': '╭────────────────────────────╮', 'slot': 'accent'}]}, {'cells': [{'text': f'│ {expression:<26} │', 'slot': 'primary'}]}, {'cells': [{'text': f'│ = {text:<24} │', 'slot': 'muted'}]}, {'cells': [{'text': '╰────────────────────────────╯', 'slot': 'accent'}]}]}, 'value': text}
json.dump({'version': 1, 'items': [item]}, sys.stdout, separators=(',', ':'))
sys.stdout.write('\n')
