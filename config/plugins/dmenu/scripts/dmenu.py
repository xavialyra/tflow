#!/usr/bin/env python3
import json
import sys


class DmenuError(Exception):
    pass


def scalar_option(options, name, default=None):
    value = options.get(name, default)
    if isinstance(value, list):
        raise DmenuError(f"--{name} may be specified only once")
    return value


def bool_option(options, name):
    value = scalar_option(options, name, False)
    if not isinstance(value, bool):
        raise DmenuError(f"--{name} must be a boolean flag")
    return value


def string_option(options, name):
    value = scalar_option(options, name)
    if value is None:
        return None
    if not isinstance(value, str):
        raise DmenuError(f"--{name} must be a string")
    return value


def parse_delimiter(options):
    value = string_option(options, "nth-delimiter")
    if value is None:
        return ("character", "\t")
    if value == "whitespace":
        return ("whitespace", " ")
    if len(value) != 1 or ord(value) >= 128:
        raise DmenuError(
            "--nth-delimiter must be a single ASCII character or 'whitespace'"
        )
    return ("character", value)


def parse_index(value, source):
    try:
        index = int(value.strip(), 10)
    except ValueError as error:
        raise DmenuError(
            f"invalid field selector {value!r} in field format {source!r}"
        ) from error
    if index == 0:
        raise DmenuError("field indexes start at 1; use 0 to disable a field format")
    if index < 0 or index > sys.maxsize:
        raise DmenuError(
            f"invalid field selector {value!r} in field format {source!r}"
        )
    return index


def parse_selector(token, source):
    token = token.strip()
    if not token:
        raise DmenuError(f"field format {source!r} contains an empty selector")
    if ".." in token:
        start_source, end_source = token.split("..", 1)
        start = parse_index(start_source, source)
        end = parse_index(end_source, source) if end_source else None
        if end is not None and start > end:
            raise DmenuError(f"field range {token!r} is backwards")
        return ("range", start, end)
    return ("single", parse_index(token, source))


def parse_template(source):
    parts = []
    literal = []
    position = 0
    while position < len(source):
        character = source[position]
        if character == "{":
            if literal:
                parts.append(("literal", "".join(literal)))
                literal = []
            end = source.find("}", position + 1)
            if end < 0:
                raise DmenuError(f"field format {source!r} has an unclosed '{{'")
            parts.append(("selector", parse_selector(source[position + 1 : end], source)))
            position = end + 1
        elif character == "}":
            raise DmenuError(f"field format {source!r} has an unmatched '}}'")
        else:
            literal.append(character)
            position += 1
    if literal:
        parts.append(("literal", "".join(literal)))
    return ("template", parts)


def parse_format(value):
    if value is None:
        return None
    if value.strip() == "0":
        return None
    if all(character.isascii() and (character.isdigit() or character in ",.") for character in value):
        selectors = [parse_selector(token, value) for token in value.split(",")]
        if not selectors:
            raise DmenuError("field format must not be empty")
        return ("fields", selectors)
    return parse_template(value)


def split_fields(text, delimiter):
    kind, value = delimiter
    if kind == "whitespace":
        return text.split()
    return text.split(value)


def select_fields(fields, selector):
    if selector[0] == "single":
        index = selector[1] - 1
        return fields[index : index + 1] if index < len(fields) else []
    first = selector[1] - 1
    last = min(selector[2] if selector[2] is not None else len(fields), len(fields))
    return fields[first:last] if first < last else []


def render_format(field_format, text, delimiter, joiner):
    if field_format is None:
        return text
    fields = split_fields(text, delimiter)
    if field_format[0] == "fields":
        selected = []
        for selector in field_format[1]:
            selected.extend(select_fields(fields, selector))
        return joiner.join(selected)
    output = []
    for part in field_format[1]:
        if part[0] == "literal":
            output.append(part[1])
        else:
            output.append(joiner.join(select_fields(fields, part[1])))
    return "".join(output)


def sanitize(text):
    output = []
    escape = False
    csi = False
    osc = False
    osc_escape = False
    for character in text:
        if osc:
            if osc_escape:
                osc_escape = False
                if character == "\\":
                    osc = False
            elif character == "\a":
                osc = False
            elif character == "\x1b":
                osc_escape = True
            continue
        if csi:
            if "@" <= character <= "~":
                csi = False
            continue
        if escape:
            escape = False
            if character == "[":
                csi = True
            elif character == "]":
                osc = True
            continue
        if character == "\x1b":
            escape = True
        elif ord(character) < 32 or 127 <= ord(character) <= 159:
            if character == "\t":
                output.append(" ")
        else:
            output.append(character)
    return "".join(output)


def strip_rofi_metadata(record):
    return record.split(b"\0", 1)[0]


def read_candidates(path, nul_records):
    if path is None:
        return []
    with open(path, "rb") as source:
        data = source.read()
    if not data:
        return []
    separator = b"\0" if nul_records else b"\n"
    if data.endswith(separator):
        data = data[: -len(separator)]
    candidates = []
    for index, record in enumerate(data.split(separator)):
        if not nul_records and record.endswith(b"\r"):
            record = record[:-1]
        text_bytes = record if nul_records else strip_rofi_metadata(record)
        candidates.append(
            {
                "index": index,
                "raw": text_bytes,
                "text": text_bytes.decode("utf-8", errors="replace"),
            }
        )
    return candidates


def matches_query(text, query):
    folded = text.lower()
    return all(token.lower() in folded for token in query.split())


def invocation_path(descriptor):
    if not isinstance(descriptor, dict):
        raise DmenuError("dmenu View requires an stdin descriptor")
    path = descriptor.get("path")
    if path is None and descriptor.get("is_tty") is True:
        return None
    if not isinstance(path, str) or not path:
        raise DmenuError("dmenu View received an invalid captured stdin path")
    return path


def item_mode(context):
    query = context.get("query", {})
    if not isinstance(query, dict):
        raise DmenuError("invalid dmenu items context")
    options = query
    query = query.get("initial", "")
    if not isinstance(query, str):
        raise DmenuError("invalid dmenu query")
    query = sanitize(query)
    delimiter = parse_delimiter(options)
    with_nth = parse_format(string_option(options, "with-nth"))
    match_nth = parse_format(string_option(options, "match-nth"))
    candidates = read_candidates(
        invocation_path(context.get("input", {}).get("stdin")),
        bool_option(options, "dmenu0"),
    )
    items = []
    for candidate in candidates:
        display = sanitize(render_format(with_nth, candidate["text"], delimiter, " "))
        searchable = sanitize(
            render_format(match_nth, candidate["text"], delimiter, " ")
            if match_nth is not None
            else display
        )
        if matches_query(searchable, query):
            item = {"label": display, "value": str(candidate["index"])}
            if not display:
                item["allow_empty"] = True
            items.append(item)
    json.dump(items, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")
    return 0


def result_mode(context):
    options = context.get("options", {})
    selected = context.get("selected")
    typed = context.get("typed", "")
    if not isinstance(options, dict):
        raise DmenuError("dmenu completion options must be an object")
    if selected is not None and not isinstance(selected, dict):
        raise DmenuError("dmenu selected item must be an object or null")

    candidate_index = selected.get("value") if selected is not None else None
    nul_records = bool_option(options, "dmenu0")
    separator = b"\0" if nul_records else b"\n"
    if candidate_index is None:
        if not isinstance(typed, str):
            raise DmenuError("dmenu unmatched input must be a string")
        output = sanitize(typed).encode("utf-8")
    else:
        try:
            index = int(candidate_index, 10)
        except (TypeError, ValueError) as error:
            raise DmenuError("dmenu selected item has an invalid index") from error
        candidates = read_candidates(invocation_path(context.get("stdin")), nul_records)
        if index < 0 or index >= len(candidates):
            raise DmenuError(f"dmenu selected item index {index} is out of range")
        candidate = candidates[index]
        if bool_option(options, "index"):
            output = str(candidate["index"]).encode("ascii")
        else:
            accept_nth = parse_format(string_option(options, "accept-nth"))
            if accept_nth is None:
                output = candidate["raw"]
            else:
                delimiter = parse_delimiter(options)
                joiner = delimiter[1]
                output = render_format(
                    accept_nth, candidate["text"], delimiter, joiner
                ).encode("utf-8")
    sys.stdout.buffer.write(output)
    sys.stdout.buffer.write(separator)
    return 0


def main():
    try:
        context = json.load(sys.stdin)
        if len(sys.argv) != 2:
            raise DmenuError("expected items or result mode")
        if sys.argv[1] == "items":
            return item_mode(context)
        if sys.argv[1] == "result":
            return result_mode(context)
        raise DmenuError(f"unsupported mode {sys.argv[1]!r}")
    except (DmenuError, OSError, json.JSONDecodeError) as error:
        print(f"dmenu: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
