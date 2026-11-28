#!/usr/bin/env python3
"""Add the two stock shutter contacts to the isolated X-Pro2 scene graph."""
import argparse
import json
from pathlib import Path


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('graph', type=Path)
parser.add_argument('output', type=Path, help='New file for the augmented graph')
args = parser.parse_args()

document = json.loads(args.graph.read_text())
graph = document['graph']
if graph['firmwareId'] != 'xpro2-511':
    parser.error('expected an X-Pro2 5.11 graph')
gpio, = (node for node in graph['devices']['nodes'] if node['model']['kind'] == 'gpio')
if gpio['model']['base'] != 0xfff0c000 or gpio['model']['inputs'][0] & 3 != 3:
    parser.error('expected released S1/S2 inputs on XPP GPIO0.0 and GPIO0.1')

positions = ['released', 'pressed']
buttons = [('shutter_half', 'Half press', 2, 0xe29b08),
           ('shutter_full', 'Full press', 3, 0xe29b24)]
ids = [button[0] for button in buttons]
if any(item['id'] in ids for item in graph['controls']['controls']):
    parser.error('the graph already defines a shutter control')
for control, label, key, row in buttons:
    graph['controls']['controls'].append({
        'id': control, 'label': label, 'kind': 'button', 'positions': positions,
        'initial': 'released', 'unavailable': None,
    })
    driver = graph['contactDriver']
    driver['catalog'].append({'id': control, 'label': label, 'kind': 'button', 'supported': True})
    driver['actuators'].append({
        'id': control, 'momentary': True,
        'states': [{
            'position': position,
            'ready': [{'address': row, 'bytes': [key], 'pointer': None}],
            'expected': [{'address': row + 21, 'bytes': [index], 'pointer': None}],
        } for index, position in enumerate(positions)],
    })

cases = []
for half in positions:
    for full in positions:
        # Full travel also closes the first stage of the physical switch.
        levels = [half == full == 'released', full == 'released']
        cases.append({
            'positions': [half, full],
            'signals': [{
                'kind': 'gpio', 'node': gpio['name'], 'pin': {'port': 0, 'pin': pin},
                'high': high,
            } for pin, high in enumerate(levels)],
        })
graph['controls']['circuits'].append({'controls': ids, 'cases': cases})
with args.output.open('x') as output:
    output.write(json.dumps(document, indent=2) + '\n')
