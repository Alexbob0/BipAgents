from bipbridge.hermes import SSEParser


def parse(text: str):
    parser = SSEParser()
    events = []
    for line in text.split("\n"):
        ev = parser.feed_line(line)
        if ev:
            events.append(ev)
    last = parser.finish()
    if last:
        events.append(last)
    return events


def test_type_from_json_then_event_line():
    events = parse(
        ': keepalive\n\n'
        'event: message.delta\ndata: {"event": "message.delta", "run_id": "r1", "delta": "Bon"}\n\n'
        'data: {"type": "assistant.delta", "delta": "jour"}\n\n'
        'event: run.completed\ndata: {"run_id": "r1"}\n\n'
    )
    assert [e.type for e in events] == ["message.delta", "assistant.delta", "run.completed"]
    assert [e.text for e in events[:2]] == ["Bon", "jour"]
    assert events[2].data["run_id"] == "r1"


def test_keepalive_only_yields_nothing():
    assert parse(": keepalive\n\n: keepalive\n\n") == []


def test_multiline_data_and_done():
    events = parse('data: {"type": "x",\ndata:  "delta": "a"}\n\ndata: [DONE]\n\n')
    assert events[0].type == "x" and events[0].text == "a"
    assert events[1].type == "done"


def test_bare_json_line_and_plain_text_data():
    events = parse('{"type": "message.delta", "delta": "hi"}\nevent: note\ndata: plain words\n\n')
    assert events[0].type == "message.delta" and events[0].text == "hi"
    assert events[1].type == "note" and events[1].data == {"text": "plain words"}


def test_nested_delta_and_final_text():
    events = parse('data: {"type": "message.delta", "delta": {"content": "x"}}\n\n'
                   'data: {"type": "run.completed", "output": "Réponse finale."}\n\n')
    assert events[0].text == "x"
    assert events[1].final_text == "Réponse finale."
