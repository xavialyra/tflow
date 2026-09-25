#!/usr/bin/env python3
"""Safe mathematical expression evaluator for the calculator workflow.

Parses input expressions using Python's AST without running arbitrary code.
Supports basic arithmetic, powers, modulo, bitwise operations, math constants,
and common mathematical functions.
"""
import ast
import json
import math
import operator
import sys

# Allowed binary operators
BINARY_OPS = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod,
    ast.Pow: operator.pow,
    ast.BitAnd: operator.and_,
    ast.BitOr: operator.or_,
    ast.BitXor: operator.xor,
    ast.LShift: operator.lshift,
    ast.RShift: operator.rshift,
}

# Allowed unary operators
UNARY_OPS = {
    ast.UAdd: operator.pos,
    ast.USub: operator.neg,
    ast.Invert: operator.invert,
}

# Allowed constants
MATH_CONSTANTS = {
    "pi": math.pi,
    "e": math.e,
    "tau": math.tau,
}

# Allowed mathematical functions
MATH_FUNCTIONS = {
    "abs": abs,
    "round": round,
    "sqrt": math.sqrt,
    "cbrt": math.cbrt if hasattr(math, "cbrt") else (lambda x: x ** (1 / 3)),
    "ceil": math.ceil,
    "floor": math.floor,
    "log": math.log,
    "log2": math.log2,
    "log10": math.log10,
    "exp": math.exp,
    "sin": math.sin,
    "cos": math.cos,
    "tan": math.tan,
    "degrees": math.degrees,
    "radians": math.radians,
    "factorial": math.factorial,
    "gcd": math.gcd,
    "hex": lambda x: hex(int(x)),
    "bin": lambda x: bin(int(x)),
    "oct": lambda x: oct(int(x)),
}


def safe_eval(node):
    """Recursively evaluate an AST node within strict safety boundaries."""
    if isinstance(node, ast.Expression):
        return safe_eval(node.body)

    if isinstance(node, ast.Constant):
        if isinstance(node.value, (int, float, complex)):
            return node.value
        raise ValueError("unsupported literal type")

    if isinstance(node, ast.Name):
        if node.id in MATH_CONSTANTS:
            return MATH_CONSTANTS[node.id]
        raise ValueError(f"unknown identifier: {node.id}")

    if isinstance(node, ast.UnaryOp):
        op_func = UNARY_OPS.get(type(node.op))
        if op_func is None:
            raise ValueError(f"unsupported unary operator: {type(node.op)}")
        operand = safe_eval(node.operand)
        return op_func(operand)

    if isinstance(node, ast.BinOp):
        op_func = BINARY_OPS.get(type(node.op))
        if op_func is None:
            raise ValueError(f"unsupported binary operator: {type(node.op)}")

        left = safe_eval(node.left)
        right = safe_eval(node.right)

        # Guard against computational explosion with huge exponents / shifts
        if isinstance(node.op, ast.Pow):
            if isinstance(right, (int, float)) and right > 10_000:
                raise ValueError("exponent too large")
            if isinstance(left, (int, float)) and abs(left) > 1000 and right > 100:
                raise ValueError("result would overflow")
        elif isinstance(node.op, (ast.LShift, ast.RShift)):
            if isinstance(right, int) and (right < 0 or right > 1000):
                raise ValueError("shift count out of bounds")

        return op_func(left, right)

    if isinstance(node, ast.Call):
        if not isinstance(node.func, ast.Name) or node.func.id not in MATH_FUNCTIONS:
            raise ValueError("unsupported function call")
        func = MATH_FUNCTIONS[node.func.id]
        if node.keywords:
            raise ValueError("keyword arguments not allowed")
        args = [safe_eval(arg) for arg in node.args]
        return func(*args)

    raise ValueError(f"unsupported expression node: {type(node)}")


def format_result(value) -> tuple[str, dict]:
    """Format evaluated value into primary string and metadata."""
    metadata = {}
    if isinstance(value, int):
        text = str(value)
        metadata = {
            "type": "integer",
            "decimal": text,
            "hex": hex(value),
            "oct": oct(value),
            "bin": bin(value),
        }
    elif isinstance(value, float):
        if value.is_integer() and abs(value) < 1e15:
            int_val = int(value)
            text = str(int_val)
            metadata = {
                "type": "integer",
                "decimal": text,
                "hex": hex(int_val),
                "oct": oct(int_val),
                "bin": bin(int_val),
            }
        else:
            text = f"{value:.10g}"
            metadata = {"type": "float", "decimal": text}
    else:
        text = str(value)
        metadata = {"type": "string", "value": text}

    return text, metadata


def build_card_display(expression: str, result_text: str, metadata: dict) -> dict:
    """Construct an aligned card box for the picker list."""
    res_line = f"= {result_text}"

    pad_width = max(len(expression), len(res_line), 26)
    pad_width = min(pad_width, 60)  # Bound to sensible max width

    top_border = "╭" + "─" * (pad_width + 2) + "╮"
    bot_border = "╰" + "─" * (pad_width + 2) + "╯"

    expr_padded = expression[:pad_width].ljust(pad_width)
    res_padded = res_line[:pad_width].ljust(pad_width)

    return {
        "rows": [
            {"cells": [{"text": top_border, "slot": "accent"}]},
            {"cells": [{"text": f"│ {expr_padded} │", "slot": "primary"}]},
            {"cells": [{"text": f"│ {res_padded} │", "slot": "muted"}]},
            {"cells": [{"text": bot_border, "slot": "accent"}]},
        ]
    }


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        json.dump({"version": 1, "items": []}, sys.stdout)
        sys.stdout.write("\n")
        return

    state = request.get("context", {}).get("engine", {}).get("state", {})
    expression = state.get("input", "").strip()
    if not expression:
        json.dump({"version": 1, "items": []}, sys.stdout)
        sys.stdout.write("\n")
        return

    try:
        parsed = ast.parse(expression, mode="eval")
        result = safe_eval(parsed)
        result_text, metadata = format_result(result)
    except Exception:
        # Syntax error, div by zero, unsupported node, etc. -> no candidate items
        json.dump({"version": 1, "items": []}, sys.stdout, separators=(",", ":"))
        sys.stdout.write("\n")
        return

    item = {
        "display": build_card_display(expression, result_text, metadata),
        "value": result_text,
        "metadata": metadata,
    }

    json.dump({"version": 1, "items": [item]}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
