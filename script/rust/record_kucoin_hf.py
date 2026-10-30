#!/usr/bin/env python3
"""Generate HF-0 contract fixtures from public documentation and pinned SDK models.

No exchange API, credentials, Rails process or production implementation is used.
  python3 script/rust/record_kucoin_hf.py --source-dir /tmp/kucoin-hf-sources
  python3 script/rust/record_kucoin_hf.py --source-dir /tmp/kucoin-hf-sources --check
Missing source files are fetched from the URLs below. Retain the cache to reproduce
this dated snapshot: documentation is mutable. --check never fetches or writes.
"""
import argparse
import base64
import copy
import hashlib
import hmac
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'rust/tests/fixtures/kucoin_hf'
REV = 'c2f39bdb8b7d5f31cfb3fd532f9baf2f09fd8f12'
DATE = '2026-10-30'
DOCS = {
    'mode': 'v2/rest/ua/get-account-mode',
    'opened': 'rest/account-info/account-funding/get-account-type-spot',
    'balances': 'rest/account-info/account-funding/get-account-list-spot',
    'apikey': 'rest/account-info/account-funding/get-apikey-info',
    'place': 'rest/spot-trading/orders/add-order',
    'status_id': 'rest/spot-trading/orders/get-order-by-orderld',
    'status_client': 'rest/spot-trading/orders/get-order-by-clientoid',
    'fills': 'rest/spot-trading/orders/get-trade-history',
    'cancel': 'rest/spot-trading/orders/cancel-order-by-orderld',
    'errors': 'error-code/spot',
    'http_errors': 'error-code/http',
    'rate_limit': 'rate-limit-rule-classic',
    'authentication': 'authentication',
}
SDK = {
    'sdk_fills_request': 'generate/spot/order/model_get_trade_history_req.py',
    'sdk_fills_response': 'generate/spot/order/model_get_trade_history_resp.py',
    'sdk_fills_item': 'generate/spot/order/model_get_trade_history_items.py',
    'sdk_balances': 'generate/account/account/model_get_spot_account_list_data.py',
    'sdk_status': 'generate/spot/order/model_get_order_by_order_id_resp.py',
    'sdk_signer': 'internal/infra/default_signer.py',
}
URLS = {k: 'https://www.kucoin.com/docs-new/' + v for k, v in DOCS.items()}
URLS.update({k: f'https://raw.githubusercontent.com/Kucoin/kucoin-universal-sdk/{REV}/sdk/python/kucoin_universal_sdk/{v}' for k, v in SDK.items()})
OID = '670fd33bf9406e0007ab3945'
UUID = '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01'
SYMBOL = 'BTC-USDT'
AT = 1729577515000
END = AT + 1000


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def encode(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode()


class Page(HTMLParser):
    def __init__(self, html):
        super().__init__()
        self.blocks, self.text, self.block, self.skip = [], [], None, 0
        self.feed(html)

    def handle_starttag(self, tag, attrs):
        if tag in ('script', 'style'):
            self.skip += 1
        if tag == 'pre':
            self.block = []

    def handle_endtag(self, tag):
        if tag in ('script', 'style'):
            self.skip -= 1
        if tag == 'pre' and self.block is not None:
            self.blocks.append(''.join(self.block))
            self.block = None

    def handle_data(self, text):
        if not self.skip:
            self.text.append(text)
        if self.block is not None:
            self.block.append(text)


def request(endpoint, query=None, body=None, path=None):
    method, route = ROUTES[endpoint]
    r = {'method': method, 'path': path or route.replace('{orderId}', OID).replace('{clientOid}', UUID), 'query': query or []}
    if body is not None:
        r['body'] = body
    return r


ROUTES = {
    'mode': ('GET', '/api/ua/v2/account/mode'),
    'opened': ('GET', '/api/v1/hf/accounts/opened'),
    'balances': ('GET', '/api/v1/accounts'),
    'apikey': ('GET', '/api/v1/user/api-key'),
    'place': ('POST', '/api/v1/hf/orders'),
    'status_id': ('GET', '/api/v1/hf/orders/{orderId}'),
    'status_client': ('GET', '/api/v1/hf/orders/client-order/{clientOid}'),
    'fills': ('GET', '/api/v1/hf/fills'),
    'cancel': ('DELETE', '/api/v1/hf/orders/{orderId}'),
}


def response(data, code='200000', status=200):
    return {'status': status, 'body': {'code': code, 'data': data}}


def delta(before, after, path=''):
    """Exact JSON-pointer edits, including removals, relative to the published body."""
    if isinstance(before, dict) and isinstance(after, dict):
        out = []
        for k in sorted(set(before) | set(after)):
            p = path + '/' + k.replace('~', '~0').replace('/', '~1')
            if k not in after:
                out.append({'path': p, 'remove': True})
            elif k not in before:
                out.append({'path': p, 'value': after[k]})
            else:
                out.extend(delta(before[k], after[k], p))
        return out
    return [] if before == after else [{'path': path or '/', 'value': after}]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-dir', type=Path, required=True)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    sources, pages, examples = {}, {}, {}
    for key, url in URLS.items():
        file = args.source_dir / (key + ('.py' if key in SDK else '.html'))
        if not file.exists() and not args.check:
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(urllib.request.urlopen(url, timeout=45).read())
        raw = file.read_bytes()
        sources[key] = {'url': url, 'fetched_date': DATE, 'sha256': sha(raw)}
        if key in SDK:
            sources[key]['revision'] = REV
            continue
        page = Page(raw.decode())
        pages[key] = page
        if key in ROUTES:
            bodies = []
            for block in page.blocks:
                try:
                    parsed = json.loads(block)
                    if isinstance(parsed, dict) and parsed.get('code') == '200000':
                        bodies.append(parsed)
                except json.JSONDecodeError:
                    pass
            # API-key docs deliberately contain an unquoted redacted uid, not valid JSON.
            if key != 'apikey':
                assert len(bodies) == 1, (key, bodies)
            examples[key] = {'source': key, 'request_examples': [b for b in page.blocks if not b.lstrip().startswith('{')],
                             'response_text': next(b for b in reversed(page.blocks) if b.lstrip().startswith('{'))}
            if bodies:
                examples[key]['body'] = bodies[0]
    assert 'last_id: Optional[int]' in (args.source_dir / 'sdk_fills_request.py').read_text()
    assert 'trade_id: Optional[int]' in (args.source_dir / 'sdk_fills_item.py').read_text()
    assert 'holds: Optional[str]' in (args.source_dir / 'sdk_balances.py').read_text()
    assert 'active: Optional[bool]' in (args.source_dir / 'sdk_status.py').read_text()
    assert 'not been URL-encoded' in ''.join(pages['authentication'].text)

    def variant(id, endpoint, reply, expect, note, query=None, body=None, extra_sources=()):
        c = {'id': id, 'endpoint': endpoint,
             'provenance': {'kind': 'synthetic-variant', 'sources': [endpoint, *extra_sources], 'note': note},
             'request': request(endpoint, query, body), 'response': reply, 'expect': expect}
        if 'body' in reply and 'body' in examples[endpoint]:
            c['provenance']['response_edits'] = delta(examples[endpoint]['body'], reply['body'])
        return c

    account = {'examples': {k: examples[k] for k in ['mode', 'opened', 'balances', 'apikey']}, 'cases': []}
    for id, mode, opened, result in [('detect_classic', 'CLASSIC', False, 'ClassicLf'), ('detect_hf', 'CLASSIC', True, 'ClassicHf'), ('detect_uta', 'UNIFIED', None, 'UnsupportedUta')]:
        c = variant(id, 'mode', response({'selfAccountMode': mode}), {'mode': result, 'placement_posts': 0, 'opened_requests': int(opened is not None)},
                    'Keep only selfAccountMode; vary CLASSIC/UNIFIED and the separately returned opened boolean.', extra_sources=('opened',))
        if opened is not None:
            c['then'] = {'request': request('opened'), 'response': response(opened)}
        account['cases'].append(c)
    for id, endpoint, reply in [
        ('mode_unknown', 'mode', response({'selfAccountMode': 'FUTURE'})),
        ('mode_missing', 'mode', response({})),
        ('mode_timeout', 'mode', {'transport_error': 'MaybeSent'}),
        ('mode_401', 'mode', response(None, '400003', 401)),
        ('mode_403', 'mode', response(None, '400007', 403)),
        ('mode_429', 'mode', response(None, '429000', 429)),
        ('mode_error_envelope', 'mode', response({'selfAccountMode': 'CLASSIC'}, '500000')),
        ('opened_null', 'opened', response(None)),
        ('opened_string', 'opened', response('true')),
        ('opened_number', 'opened', response(1)),
        ('opened_object', 'opened', response({'opened': True})),
        ('opened_absent', 'opened', {'status': 200, 'body': {'code': '200000'}}),
        ('opened_error_envelope', 'opened', response(True, '500000')),
    ]:
        c = variant(id, endpoint, reply, {'mode': 'Unknown', 'placement_posts': 0, 'placement_intents_created': 0, 'preserve_stored_balances': True},
                    'Fault injection; no fallback mode, no new intent and no synthetic balance. Transport/status are synthetic.')
        if endpoint == 'opened':
            c['given'] = {'selfAccountMode': 'CLASSIC'}
        account['cases'].append(c)
    bal = {'id': '548674591753', 'currency': 'USDT', 'type': 'trade_hf', 'balance': '26.66759503', 'available': '25.12345678', 'holds': '1.54413825'}
    for hold in ['holds', 'locked']:
        b = copy.deepcopy(bal)
        b[hold] = b.pop('holds')
        account['cases'].append(variant('balances_' + hold, 'balances', response([b]),
            {'placement_posts': 0, 'available': '25.12345678', 'tracker_total': '26.66759503'},
            'Select trade_hf, introduce nonzero held funds. locked is a compatibility alias policy, not a documented SDK field.',
            [['type', 'trade_hf']], extra_sources=('sdk_balances',)))
    account['cases'].append(variant('balances_fail_503', 'balances', {'status': 503, 'raw_body': 'unavailable'},
        {'placement_posts': 0, 'placement_intents_created': 0, 'preserve_stored_balances': True}, 'Read failure must preserve stored balances.', [['type', 'trade_hf']]))
    for id, permission, allowed in [('permissions_spot', 'General,Spot', True), ('permissions_general_only', 'General', False)]:
        account['cases'].append(variant(id, 'apikey', response({'permission': permission}), {'placement_posts': 0, 'trading_allowed': allowed},
            'Minimal SDK-shaped permission response; omit all unrelated fields, including the invalid redacted uid in the source example.'))

    orders = {'identity': {'orderId': OID, 'clientOid': UUID, 'symbol': SYMBOL, 'side': 'buy', 'type': 'limit'},
              'examples': {k: examples[k] for k in ['place', 'status_id', 'status_client', 'cancel']}, 'cases': []}
    for side in ['buy', 'sell']:
        for shape in ['market_size', 'market_funds', 'limit']:
            body = {'clientOid': UUID, 'symbol': SYMBOL, 'side': side, 'type': 'limit' if shape == 'limit' else 'market'}
            body.update({'funds': '1'} if shape == 'market_funds' else {'size': '0.00001'})
            if shape == 'limit':
                body['price'] = '67717.6'
            orders['cases'].append(variant(f'place_{side}_{shape}', 'place', response({'orderId': OID, 'clientOid': UUID}),
                {'classification': 'Accepted', 'placement_posts': 1, 'transaction_rows_delta': 1, 'intent': 'transferred_to_row', 'raw_order_id': OID},
                'Persisted UUID replaces example clientOid; explicit buy/sell size/funds/limit request variants. Values are already sized; HF-2 must reuse classic sizing.', body=body))
    base = copy.deepcopy(examples['status_id']['body']['data'])
    base.update(id=OID, clientOid=UUID, price='67717.6', funds='0.677176', createdAt=AT,
                lastUpdatedAt=AT + 100, cancelExist=False, active=False, inOrderBook=False,
                cancelledSize='0', cancelledFunds='0', remainSize='0', remainFunds='0', cancelReason=0)
    status_cases = [
        ('status_active_before_book', {'active': True, 'remainSize': '0.00001', 'remainFunds': '0.677176'}, 'Open'),
        ('status_active_in_book', {'active': True, 'inOrderBook': True, 'remainSize': '0.00001', 'remainFunds': '0.677176'}, 'Open'),
        ('status_filled', {'dealSize': '0.00001', 'dealFunds': '0.677176', 'fee': '0.000677176'}, 'Closed'),
        ('status_cancelled_partial', {'cancelExist': True, 'dealSize': '0.000004', 'dealFunds': '0.2708704', 'fee': '0.0002708704', 'cancelledSize': '0.000006', 'cancelledFunds': '0.4063056'}, 'Cancelled'),
        ('status_missing_active', {'active': None}, 'Unresolved'),
        ('status_string_active', {'active': 'false'}, 'Unresolved'),
        ('status_inactive_in_book', {'inOrderBook': True}, 'Unresolved'),
        ('status_missing_execution', {'dealFunds': None}, 'Unresolved'),
        ('status_conflicting_execution', {'dealSize': '0', 'dealFunds': '1'}, 'Unresolved'),
        ('status_price_round_half_up', {'dealSize': '0.00001', 'dealFunds': '0.6771765', 'funds': '0.6771765', 'price': '67717.65'}, 'Closed'),
        ('status_zero_59s', {'type': 'market', 'price': '0'}, 'Unknown'),
        ('status_zero_60s', {'type': 'market', 'price': '0'}, 'Unknown'),
        ('status_zero_after_60s', {'type': 'market', 'price': '0'}, 'Closed'),
        ('status_zero_limit', {}, 'Unknown'),
    ]
    for id, changes, state in status_cases:
        data = copy.deepcopy(base)
        for k, v in changes.items():
            if v is None:
                del data[k]
            else:
                data[k] = v
        expect = {'status': state, 'gross_base': data.get('dealSize'), 'gross_quote': data.get('dealFunds'), 'reserve_credit': '0'}
        # Reserve effects require the engine in HF-4; this is only a read/parser contract.
        if id == 'status_price_round_half_up':
            expect.update(price='67717.7', price_decimals=1)
        c = variant(id, 'status_id', response(data), expect,
                    'Coherent immutable identity, timestamp and execution/remaining/cancelled amounts; typed HF flag and zero-fill policy variants. No parser execution implied.',
                    [['symbol', SYMBOL]], extra_sources=('sdk_status',))
        c['given'] = {'now_ms': AT + (59000 if id == 'status_zero_59s' else 60001 if id == 'status_zero_after_60s' else 60000), 'price_decimals': 1}
        orders['cases'].append(c)
    for endpoint in ['status_id', 'status_client']:
        data = copy.deepcopy(base)
        data.update(active=True, remainSize='0.00001', remainFunds='0.677176')
        orders['cases'].append(variant('lookup_matching_' + endpoint, endpoint, response(data),
            {'identity_matches': True, 'recovered': True, 'placement_posts': 0, 'transaction_rows_delta': 1, 'intent': 'transferred_to_row'},
            'Coherent positive lookup by each identifier; acceptance is recovered once, without another POST.', [['symbol', SYMBOL]]))
    for id, field, value in [('lookup_wrong_id', 'id', 'different-order'), ('lookup_wrong_client_oid', 'clientOid', 'different-intent'),
                             ('lookup_wrong_symbol', 'symbol', 'ETH-USDT'), ('lookup_wrong_side', 'side', 'sell'), ('lookup_wrong_type', 'type', 'market'), ('lookup_missing', None, None)]:
        data = copy.deepcopy(base)
        if field:
            data[field] = value
        else:
            data = None
        endpoint = 'status_id' if field == 'id' else 'status_client'
        orders['cases'].append(variant(id, endpoint, response(data), {'intent': 'retained', 'placement_posts': 0, 'transaction_rows_delta': 0},
            'Identity mismatch or absent lookup is not proof of non-placement.', [['symbol', SYMBOL]]))
    for id, reply, outcome in [
        ('cancel_ack', response({'orderId': OID}), 'acknowledged'),
        ('cancel_wrong_id', response({'orderId': 'different-order'}), 'uncertain'),
        ('cancel_refused', {'status': 400, 'body': {'code': '400100', 'msg': 'Parameter Error'}}, 'rejected'),
        ('cancel_lost_reply', {'transport_error': 'MaybeSent'}, 'uncertain'),
        ('cancel_503', {'status': 503, 'raw_body': 'unavailable'}, 'uncertain'),
        ('cancel_unreadable', {'status': 200, 'raw_body': '{'}, 'uncertain'),
    ]:
        orders['cases'].append(variant(id, 'cancel', reply, {'cancellation': outcome, 'terminal_proven': False, 'reserve_credit': '0', 'poll_required': True},
            'Cancel response concerns the request only; preserve the order/reserve until a final status read.', [['symbol', SYMBOL]]))

    errors = {'cases': []}
    text = '\n'.join(pages['errors'].text)
    codes = ['400002', '400003', '400004', '400005', '400006', '400007', '400100', '200004', '429000']
    meanings = {}
    for code in codes + ['500000', '230005']:
        if code == '429000':
            assert 'HTTP code 429 with error code 429000' in ''.join(pages['rate_limit'].text)
            meanings[code] = 'rate limit exceeded'
            continue
        page_text = text
        match = re.search(r'\n' + code + r'\n([^\n]+)', page_text)
        assert match, code
        meanings[code] = match[1]
    errors['documented_messages'] = {k: v for k, v in meanings.items() if k != '429000'}
    errors['synthetic_messages'] = {'429000': meanings['429000'], '400100_duplicate': 'clientOid already exists', '499999': 'unknown'}
    fault_rows = []
    for code in codes:
        fault_rows.append(('reject_' + code, {'status': 429 if code == '429000' else 400, 'body': {'code': code, 'msg': meanings[code]}}, 'Rejected'))
    fault_rows += [
        ('http_429', {'status': 429, 'raw_body': 'rate limited'}, 'Rejected'),
        ('duplicate_client_oid', {'status': 400, 'body': {'code': '400100', 'msg': 'clientOid already exists'}}, 'Ambiguous'),
        ('unknown_code', {'status': 400, 'body': {'code': '499999', 'msg': 'unknown'}}, 'Ambiguous'),
        ('missing_code', {'status': 200, 'body': {'data': {'orderId': OID}}}, 'Ambiguous'),
        ('success_missing_id', response({}), 'Ambiguous'),
        ('success_empty_id', response({'orderId': ''}), 'Ambiguous'),
        ('success_conflicting_client_oid', response({'orderId': OID, 'clientOid': 'another-intent'}), 'Ambiguous'),
        ('truncated_success', {'status': 200, 'raw_body': '{"code":"200000","data":'}, 'Ambiguous'),
        ('lost_reply', {'transport_error': 'MaybeSent'}, 'Ambiguous'),
        ('generic_503', {'status': 503, 'raw_body': 'unavailable'}, 'Ambiguous'),
        ('not_sent', {'transport_error': 'NotSent'}, 'NotSent'),
    ]
    for label, code, statuses in [('internal', '500000', [200, 500]), ('busy', '230005', [200, 503])]:
        for status in statuses:
            fault_rows.append((f'{label}_{status}', {'status': status, 'body': {'code': code, 'msg': meanings[code]}}, 'Ambiguous'))
    for id, reply, classification in fault_rows:
        errors['cases'].append(variant(id, 'place', reply,
            {'classification': classification, 'transaction_rows_delta': 0, 'reserve_credit': '0', 'intent': 'retained' if classification == 'Ambiguous' else 'cleared',
             'replacement_posts': 0, 'reschedule': id == 'reject_400002'},
            'Synthetic HTTP/transport placement refusal or uncertainty. Code/message pairs come from the cited error table except duplicate-clientOid, unknown and rate-limit messages; those messages are deliberate synthetic variants. Rate-limit documentation pins HTTP 429/code 429000 only. State effects are the reviewed conservative policy.',
            body={'clientOid': UUID, 'symbol': SYMBOL, 'side': 'buy', 'type': 'market', 'funds': '1'}, extra_sources=('errors', 'http_errors', 'rate_limit')))

    fills = {'examples': {'fills': examples['fills']}, 'cases': []}
    template = examples['fills']['body']['data']['items'][0]
    cursor_base = 9007199254742000
    items = []
    for i in range(102):
        item = copy.deepcopy(template)
        item.update(id=cursor_base - i, tradeId=11029373945659393 + i,
                    orderId='hf-history-sell' if i % 2 else 'hf-history-buy', createdAt=AT + 900 - i,
                    side='sell' if i % 2 else 'buy')
        items.append(item)

    def page(rows, cursor=None, last=None):
        query = [['symbol', SYMBOL], ['startAt', str(AT)], ['endAt', str(END - 1)], ['limit', '100']]
        if last is not None:
            query.append(['lastId', str(last)])
        return {'request': request('fills', query), 'response': response({'items': rows, 'lastId': cursor})}

    pages_good = [page(items[:100], items[99]['id']), page(items[99:], items[-1]['id'], items[99]['id']), page([], None, items[-1]['id'])]
    def fill_case(id, pages, complete=False, note=''):
        return {'id': id, 'endpoint': 'fills', 'provenance': {'kind': 'synthetic-variant', 'sources': ['fills', 'sdk_fills_request', 'sdk_fills_response', 'sdk_fills_item'],
                'note': 'Generate 102 fills from the documented item: sequential exact integer id/tradeId above 2^53, separate buy/sell history orders (independent of placement scenarios), alternating sides and timestamps within one whole second. Repeat the page boundary; finish only on empty items. Fault walks use two one-fill pages instead. ' + note},
                'given': {'interval_ms': [AT, END], 'now_ms': END, 'symbol': SYMBOL}, 'pages': pages,
                'expect': {'coverage': 'complete' if complete else 'incomplete', 'watermark': END if complete else 'unchanged',
                           'visible_api_rows_delta': 102 if complete else 0, 'csv_rows_deleted': 0,
                           'unique_fills': 102 if complete else None,
                           'gross_totals': ['0.00102', '69.071952', '0.069071952'] if complete else None}}
    fills['cases'].append(fill_case('pages_boundary_and_large_ids', pages_good, True))
    for id in ['cursor_repeated', 'cursor_missing', 'malformed_page', 'failed_page', 'throttle_exhausted', 'retention_expired_on_resume', 'symbol_unavailable', 'clamped_eight_day_query']:
        # Short pages must still continue; keep fault fixtures small and independently replayable.
        pages_bad = [page(items[:1], items[0]['id']), page(items[1:2], items[1]['id'], items[0]['id'])]
        second = pages_bad[1]['response']
        if id == 'cursor_repeated':
            second['body']['data']['lastId'] = items[0]['id']
        elif id == 'cursor_missing':
            del second['body']['data']['lastId']
        elif id == 'malformed_page':
            second['body']['data']['items'] = 'invalid'
        elif id == 'failed_page':
            pages_bad[1]['response'] = {'status': 503, 'raw_body': 'unavailable'}
        elif id == 'throttle_exhausted':
            pages_bad[1]['response'] = {'status': 429, 'body': {'code': '429000', 'msg': meanings['429000']}}
        elif id == 'symbol_unavailable':
            pages_bad[1]['response'] = {'status': 400, 'body': {'code': '900001', 'msg': 'symbol does not exist'}}
        elif id == 'clamped_eight_day_query':
            pages_bad = [page([], None)]
            pages_bad[0]['request']['query'][1][1] = str(END - 8 * 86400000)
        c = fill_case(id, pages_bad, note='Fault mutation: ' + id + '; captured page one remains outside the visible ledger.')
        c['expect']['error'] = id
        if id == 'retention_expired_on_resume':
            c['given'].update(resume_now_ms=END + 7 * 86400000 + 1, requests_after_resume=0)
        if id == 'clamped_eight_day_query':
            c['given']['interval_ms'] = [END - 8 * 86400000, END]
        fills['cases'].append(c)
    twins = [copy.deepcopy(items[0]), copy.deepcopy(items[0])]
    twins[1].update(symbol='ETH-USDT', id=cursor_base + 1, orderId='different-order')
    fills['cases'].append({'id': 'symbol_scoped_trade_identity', 'endpoint': 'fills',
        'provenance': {'kind': 'synthetic-variant', 'sources': ['fills', 'sdk_fills_item'], 'note': 'Two symbol walks share tradeId but have distinct cursor IDs and orders. Never deduplicate across symbols by tradeId alone.'},
        'items': twins, 'expect': {'ledger_ids': [f'{i["symbol"]}-{i["tradeId"]}' for i in twins], 'unique_fills': 2}})

    signing = {'credentials': {'key': 'HF0-EXAMPLE-KEY', 'secret': 'HF0-example-secret', 'passphrase': 'HF0 example passphrase'},
               'timestamp': '1729577515000', 'oracle': 'Python standard-library hmac/SHA256 + Base64, independent of Rust signer and Rails', 'cases': []}
    signing_inputs = [
        ('sign_get_mode', request('mode')), ('sign_get_opened', request('opened')),
        ('sign_get_balances', request('balances', [['type', 'trade_hf']])),
        ('sign_get_status_id', request('status_id', [['symbol', SYMBOL]])),
        ('sign_get_status_client', request('status_client', [['symbol', SYMBOL]])),
        ('sign_get_fills_cursor', pages_good[1]['request']),
        ('sign_post_market', orders['cases'][1]['request']), ('sign_post_limit', orders['cases'][2]['request']),
        ('sign_delete_symbol', request('cancel', [['symbol', SYMBOL]])),
        ('sign_get_unescaped_query', request('status_id', [['symbol', SYMBOL], ['remark', 'space +/%?=é']])),
    ]
    def mac(text):
        return base64.b64encode(hmac.new(signing['credentials']['secret'].encode(), text.encode(), hashlib.sha256).digest()).decode()
    for id, req in signing_inputs:
        req = copy.deepcopy(req)
        body = json.dumps(req.pop('body'), ensure_ascii=False, separators=(',', ':')) if 'body' in req else ''
        req['body_bytes'] = body
        query = '&'.join(k + '=' + v for k, v in req['query'])
        endpoint = req['path'] + ('?' + query if query else '')
        prehash = signing['timestamp'] + req['method'] + endpoint + body
        wire_query = urllib.parse.urlencode([tuple(p) for p in req['query']])
        signing['cases'].append({'id': id, 'request': req,
            'provenance': {'kind': 'synthetic-variant', 'sources': ['authentication', 'sdk_signer'],
                           'note': 'Throwaway credentials, frozen timestamp and coherent request bytes. Extra remark in the encoding probe tests authentication only, not endpoint parameter acceptance.'},
            'expect': {'prehash': prehash, 'transport_url': 'https://api.kucoin.com' + req['path'] + ('?' + wire_query if wire_query else ''),
                       'headers': {'KC-API-KEY': signing['credentials']['key'], 'KC-API-TIMESTAMP': signing['timestamp'],
                                   'KC-API-KEY-VERSION': '2', 'KC-API-SIGN': mac(prehash), 'KC-API-PASSPHRASE': mac(signing['credentials']['passphrase'])}}})
    output = {'account': account, 'orders': orders, 'fills': fills, 'errors': errors, 'signing': signing}
    generated = {name + '.json': encode(data) for name, data in output.items()}
    endpoints = {k: {'method': v[0], 'path': v[1], 'source': k} for k, v in ROUTES.items()}
    for k in ['status_id', 'status_client', 'fills', 'cancel']:
        endpoints[k]['required_query'] = ['symbol']
    endpoints['balances']['query'] = [['type', 'trade_hf']]
    endpoints['fills'].update(limit=100, cursor='returned data.lastId (integer, not tradeId)', terminal='explicit empty items',
                             retention_ms=7 * 86400000, max_interval_ms=7 * 86400000, local_interval='[start,end), whole seconds; send endAt=end-1',
                             order_id_filter='local only; SDK says orderId overrides other query parameters')
    endpoints['status_id']['retention_days'] = {'filled': 7, 'cancelled': 2, 'absence_proves_nonplacement': False}
    manifest = {'schema_version': 1, 'oracle': 'official-docs-and-sdk; synthetic variants are not exchange recordings',
                'sdk_revision': REV, 'sources': sources, 'recorder': 'script/rust/record_kucoin_hf.py',
                'recorder_sha256': sha(Path(__file__).read_bytes()), 'generated_sha256': {k: sha(v) for k, v in generated.items()},
                'endpoints': endpoints, 'case_counts': {k: len(v['cases']) for k, v in output.items()},
                'matrix': 'MATRIX.md', 'scope': 'HF-0 pure fixture/contract checks; no production path or database-effects validation yet',
                'source_adjustments': [
                    'Original code blocks are preserved as response_text/request_examples. Parsed JSON has no semantic edits.',
                    'Status-by-ID request and response IDs differ; status-by-clientOid request and response clientOids differ. Synthetic cases use the place orderId and a persisted UUID consistently.',
                    'Balance example requests main but returns trade/main. Synthetic balances explicitly select trade_hf; locked is repository compatibility policy, not an SDK field.',
                    'Fill request time bounds do not contain its response timestamp; synthetic walks use frozen whole-second bounds and endAt=end-1.',
                    'API-key example contains invalid JSON (unquoted masked uid); preserve text and derive only the permission field in synthetic cases.',
                    'Classification, no fallback, zero-fill timing, empty-page termination, half-open coverage and staging/publication are reviewed application policies, not promises made by example replies.',
                    'Per-case response_edits are JSON-pointer changes from source bodies; synthetic request, HTTP status, transport fault, clock and multi-page generation changes are explicit in each case and its provenance note.',
                ],
                'divergences': {
                    'D-KUCOIN-HF-MODE': 'HF-1/5: versioned detection, unknown/UTA refusal, balance family routing',
                    'D-KUCOIN-HF-ORDERS': 'HF-2/4: HF wire/signing/status/cancel and immutable order context',
                    'D-KUCOIN-HF-RETENTION': 'HF-4: no absence inference, atomic evidence settlement, handback barriers',
                    'D-KUCOIN-HF-LEDGER': 'HF-3: symbol cursor walk, staging, interval promotion, legacy cutoff rebuild',
                },
                'rulings': {'history_source': 'existing kucoin importer', 'export_coverage': 'first through last imported source row; outside remains a visible gap',
                            'gap_surface': 'existing tracker Import history line; no new copy or prompt',
                            'copy_removal_when_reading_ships': "remove the sentence claiming HF trades cannot be read"}}
    manifest['case_counts']['total'] = sum(manifest['case_counts'].values())
    generated['manifest.json'] = encode(manifest)
    if not args.check:
        OUT.mkdir(parents=True, exist_ok=True)
    for name, raw in generated.items():
        path = OUT / name
        if args.check:
            assert path.read_bytes() == raw, f'{name} differs; regenerate from the retained source snapshot'
        else:
            path.write_bytes(raw)
    print(('Checked' if args.check else 'Generated') + ': ' + ', '.join(f'{k}={v}' for k, v in manifest['case_counts'].items()))


if __name__ == '__main__':
    main()
