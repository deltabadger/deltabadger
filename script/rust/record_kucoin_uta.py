#!/usr/bin/env python3
"""UTA-0: KuCoin Unified Trading Account (UTA) spot fixtures from KuCoin's official docs (issue #841, plan UTA.md).

Reads a cache of the official doc pages (their `.md` form, fetched 2026-10-31), keeps each page's success example as
text, and writes coherent SPOT cases built on them into rust/tests/fixtures/kucoin_uta/. The docs' examples are mostly
FUTURES, so every case is a labelled synthetic variant; nothing here is an exchange recording. HMAC expectations come
from Python's standard library, independent of the Rust signer.

    python3 script/rust/record_kucoin_uta.py --source-dir <cache>          # write
    python3 script/rust/record_kucoin_uta.py --source-dir <cache> --check  # compare, write nothing
"""
import argparse, base64, hashlib, hmac, json, pathlib, re, sys, urllib.parse

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / 'rust/tests/fixtures/kucoin_uta'
FETCHED = '2026-10-31'
SDK = 'c2f39bdb8b7d5f31cfb3fd532f9baf2f09fd8f12'
UA = 'https://www.kucoin.com/docs-new/v2/rest/ua/'
# source id -> (cache file, official URL)
SOURCES = {
    'mode': ('v2_rest_ua_get-account-mode.md', UA + 'get-account-mode'),
    'place': ('v2_rest_ua_place-order.md', UA + 'place-order'),
    'cancel': ('v2_rest_ua_cancel-order.md', UA + 'cancel-order'),
    'detail': ('v2_rest_ua_get-order-details.md', UA + 'get-order-details'),
    'history': ('v2_rest_ua_get-order-history.md', UA + 'get-order-history'),
    'execution': ('v2_rest_ua_get-trade-history.md', UA + 'get-trade-history'),
    'balance': ('v2_rest_ua_get-account-currency-assets-uta.md', UA + 'get-account-currency-assets-uta'),
    'classic_balance': ('v2_rest_ua_classic-account-balance.md', UA + 'classic-account-balance'),
    'ledger': ('v2_rest_ua_account-ledger.md', UA + 'account-ledger'),
    'apikey_ua': ('v2_rest_ua_api-key.md', UA + 'api-key'),
    'apikey': ('classic_apikey.md', 'https://www.kucoin.com/docs-new/rest/account-info/account-funding/get-apikey-info'),
    'set_mode': ('v2_rest_ua_set-account-mode.md', UA + 'set-account-mode'),
    'errors': ('v2_error-code_uta-api.md', 'https://www.kucoin.com/docs-new/v2/error-code/uta-api'),
    'rate_limit': ('rate-limit-rule-uta.md', 'https://www.kucoin.com/docs-new/rate-limit-rule-uta'),
    'authentication': ('auth.md', 'https://www.kucoin.com/docs-new/authentication'),
    'introduction': ('introduction-uta.md', 'https://www.kucoin.com/docs-new/introduction-uta'),
    # The UTA base dispatches through the normal signed Spot transport.
    'sdk_signer': ('sdk_uta_base_api.py', f'https://raw.githubusercontent.com/Kucoin/kucoin-universal-sdk/{SDK}/sdk/python/kucoin_universal_sdk/generate/uta/base_api.py'),
}
SYMBOL, ORDER_ID, CLIENT_OID = 'BTC-USDT', '385577101236916224', '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a02'
PLACED_MS = 1764741957000
# Above 2^53: a cursor or id read through an f64 changes.
LAST_ID = 10000000138744534


def example(text):
    """A page's success example, as the docs print it (YAML text from `code: '200000'` to its `headers:`)."""
    m = re.search(r"^( *)code: '200000'\n.*?(?=^ *headers:)", text, re.S | re.M)
    return m.group(0) if m else None


def case(cid, sources, note, expect, **rest):
    return {'id': cid, **rest, 'provenance': {'kind': 'synthetic-variant', 'sources': sources, 'note': note}, 'expect': expect}


def ok(data):
    return {'code': '200000', 'data': data}


def order(status, filled, **edits):
    o = {'tradeType': 'SPOT', 'orderId': ORDER_ID, 'clientOid': CLIENT_OID, 'symbol': SYMBOL, 'side': 'BUY', 'orderType': 'LIMIT',
         'size': '0.002', 'sizeUnit': 'BASECCY', 'price': '50000', 'status': status, 'filledSize': filled, 'avgPrice': '50000' if filled != '0' else '',
         'fee': '0', 'feeCurrency': 'USDT', 'cancelSize': '0', 'orderTime': PLACED_MS * 10**6, 'updatedTime': (PLACED_MS + 1000) * 10**6}
    o.update(edits)
    return o


def execution(trade_id, size, value, fee='0.001', order_id=ORDER_ID, side='BUY', at_ms=PLACED_MS + 500, **edits):
    e = {'tradeType': 'SPOT', 'orderId': order_id, 'symbol': SYMBOL, 'orderType': 'LIMIT', 'side': side, 'tradeId': trade_id,
         'size': size, 'price': '50000', 'value': value, 'executionTime': at_ms * 10**6, 'fee': fee, 'feeCurrency': 'USDT',
         'tax': '', 'liquidityRole': 'TAKER', 'fillType': 'NORMAL', 'clientOid': ''}
    e.update(edits)
    return e


def page(items, last_id):
    return ok({'tradeType': 'SPOT', 'lastId': last_id, 'items': items})


def orders():
    place = []
    for side in ('BUY', 'SELL'):
        for kind, body in (('market_base', {'orderType': 'MARKET', 'size': '0.002', 'sizeUnit': 'BASECCY'}),
                           ('market_quote', {'orderType': 'MARKET', 'size': '100.5', 'sizeUnit': 'QUOTECCY'}),
                           ('limit', {'orderType': 'LIMIT', 'size': '0.002', 'sizeUnit': 'BASECCY', 'price': '50000'})):
            req = {'tradeType': 'SPOT', 'clientOid': CLIENT_OID, 'symbol': SYMBOL, 'side': side, **body}
            place.append(case(f'place_{side.lower()}_{kind}', ['place'], 'SPOT body from the documented fields; clientOid is the persisted intent UUID; size already sized.',
                              {'path': '/api/ua/v2/unified/order/place', 'body': req, 'outcome': 'accepted', 'intent_kept': False, 'rows': 1},
                              response=ok({'orderId': ORDER_ID, 'tradeType': 'SPOT', 'ts': PLACED_MS * 10**6, 'clientOid': CLIENT_OID})))
    unreadable = {'state': 'Unreadable', 'reserve_credit': '0'}
    status = [
        ('live', order(2, '0'), {'state': 'Open'}),
        ('partial_live', order(4, '0.001'), {'state': 'Open', 'filled_size': '0.001'}),
        ('filled', order(3, '0.002'), {'state': 'Closed', 'filled_size': '0.002'}),
        ('cancelled', order(5, '0', cancelSize='0.002'), {'state': 'Cancelled', 'filled_size': '0'}),
        ('partial_cancelled', order(6, '0.001', cancelSize='0.001'), {'state': 'Cancelled', 'filled_size': '0.001'}),
        ('filled_without_size', order(3, '0'), unreadable),
        ('partial_cancelled_without_size', order(6, '0'), unreadable),
        ('live_with_size', order(2, '0.001'), unreadable),
        ('partial_live_without_size', order(4, '0'), unreadable),
        ('cancelled_with_size', order(5, '0.001'), unreadable),
        ('status_triggered', order(1, '0'), unreadable),
        ('status_unknown', order(7, '0'), unreadable),
        ('status_string', order('3', '0.002'), unreadable),
        ('status_missing', {k: v for k, v in order(3, '0.002').items() if k != 'status'}, unreadable),
        ('filled_size_number', order(3, 0.002), unreadable),
        ('other_symbol', order(3, '0.002', symbol='ETH-USDT'), unreadable),
        ('other_side', order(3, '0.002', side='SELL'), unreadable),
        ('other_type', order(3, '0.002', orderType='MARKET'), unreadable),
        ('other_order', order(3, '0.002', orderId='385577101236916225'), unreadable),
        ('futures_trade_type', order(3, '0.002', tradeType='FUTURES'), unreadable),
    ]
    status_cases = [case(f'status_{cid}', ['detail'], 'Integer status 2-6 (SPOT); filledSize must agree with the status; identity checked against the stored record.', exp,
                         request={'path': '/api/ua/v2/unified/order/detail', 'query': [['tradeType', 'SPOT'], ['symbol', SYMBOL], ['orderId', ORDER_ID]]},
                         response=ok(o)) for cid, o, exp in status]
    a, b = execution('1943106280799', '0.0012', '60.12', fee='0.06'), execution('1943106280800', '0.0008', '40.08', fee='0.04', at_ms=PLACED_MS + 700)
    totals = [
        ('two_pages', [page([a], 11), page([b], 12), page([], 12)], '0.002', {'base': '0.002', 'quote': '100.2', 'fees': {'USDT': '0.1'}, 'price': '50100'}),
        ('boundary_repeat', [page([a], 11), page([a, b], 12), page([], 12)], '0.002', {'base': '0.002', 'quote': '100.2', 'fees': {'USDT': '0.1'}, 'price': '50100'}),
        ('conflicting_repeat', [page([a], 11), page([dict(a, value='60.13'), b], 12), page([], 12)], '0.002', {'state': 'Unreadable'}),
        ('repeat_matches_size', [page([a], 11), page([a], 12), page([], 12)], '0.0024', {'state': 'Waiting', 'why': 'one fill read twice is counted once, so it no longer matches filledSize'}),
        ('foreign_order', [page([a, execution('1943106280802', '0.0008', '40.08', order_id='385577101236916226')], 11), page([], 11)], '0.002', {'state': 'Unreadable'}),
        ('foreign_symbol', [page([a, dict(b, symbol='ETH-USDT')], 11), page([], 11)], '0.002', {'state': 'Unreadable'}),
        ('other_side', [page([a, dict(b, side='SELL')], 11), page([], 11)], '0.002', {'state': 'Unreadable'}),
        ('zero_value', [page([a, dict(b, value='0')], 11), page([], 11)], '0.002', {'state': 'Unreadable'}),
        ('missing_trade_id', [page([a, {k: v for k, v in b.items() if k != 'tradeId'}], 11), page([], 11)], '0.002', {'state': 'Unreadable'}),
        ('short_of_filled_size', [page([a], 11), page([], 11)], '0.002', {'state': 'Waiting', 'why': 'executions sum to less than filledSize'}),
        ('above_filled_size', [page([a, b], 11), page([], 11)], '0.0015', {'state': 'Unreadable'}),
    ]
    totals_cases = [case(f'totals_{cid}', ['execution', 'detail'], 'No cumulative quote on the order: quote = sum of validated, tradeId-deduplicated execution values; sizes must sum to filledSize exactly.',
                         dict(exp), filled_size=filled, pages=pages,
                         request={'path': '/api/ua/v2/unified/order/execution', 'query': [['tradeType', 'SPOT'], ['symbol', SYMBOL], ['orderId', ORDER_ID]]})
                    for cid, pages, filled, exp in totals]
    recovery = [
        case('recovery_found', ['detail'], 'clientOid lookup naming our clientOid and symbol is the order.', {'state': 'Open', 'intent_kept': False},
             request={'path': '/api/ua/v2/unified/order/detail', 'query': [['tradeType', 'SPOT'], ['symbol', SYMBOL], ['clientOid', CLIENT_OID]]}, response=ok(order(2, '0'))),
        case('recovery_other_client_oid', ['detail'], 'Another clientOid is never our order.', {'state': 'Unproven', 'intent_kept': True},
             response=ok(order(2, '0', clientOid='5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a03'))),
        case('recovery_not_found', ['detail', 'errors'], 'A miss proves nothing: cancelled no-fill orders are kept 2 h and wider ranges are silently clamped.',
             {'state': 'Unproven', 'intent_kept': True}, status=200, response={'code': '110103', 'msg': 'The orderId parameter does not exist.'}),
    ]
    cancel = [
        case('cancel_accepted', ['cancel'], 'Acknowledgement names our order; a request, never a terminal state.', {'outcome': 'requested', 'obligation_kept': True},
             request={'path': '/api/ua/v2/unified/order/cancel', 'body': {'tradeType': 'SPOT', 'symbol': SYMBOL, 'orderId': ORDER_ID}},
             response=ok({'tradeType': 'SPOT', 'orderId': ORDER_ID, 'clientOid': CLIENT_OID, 'ts': (PLACED_MS + 2000) * 10**6})),
        case('cancel_other_order', ['cancel'], 'An acknowledgement for another id cannot be told.', {'outcome': 'ambiguous', 'obligation_kept': True},
             response=ok({'tradeType': 'SPOT', 'orderId': '385577101236916225', 'clientOid': CLIENT_OID, 'ts': 0})),
        case('cancel_refused', ['cancel', 'errors'], 'An error envelope refuses the cancel; the order stays open.', {'outcome': 'refused', 'obligation_kept': True},
             status=200, response={'code': '110103', 'msg': 'The orderId parameter does not exist.'}),
        case('cancel_server_error', ['cancel', 'errors'], 'A 5xx or lost reply cannot be told.', {'outcome': 'ambiguous', 'obligation_kept': True},
             status=504, response={'code': '504000', 'msg': 'Gateway timeout.'}),
    ]
    return {'identity': {'orderId': ORDER_ID, 'clientOid': CLIENT_OID, 'symbol': SYMBOL, 'tradeType': 'SPOT'},
            'cases': place + status_cases + totals_cases + recovery + cancel}


def errors():
    def e(cid, status, body, outcome, note):
        intent = outcome == 'ambiguous'
        return case(f'place_{cid}', ['place', 'errors'], note, {'outcome': outcome, 'intent_kept': intent, 'rows': 0, 'replacement_blocked': intent},
                    status=status, response=body)
    def code(c, m): return {'code': c, 'msg': m}
    return {'cases': [
        e('key_missing', 200, code('400003', 'The API key does not exist or site mismatch.'), 'rejected', 'Documented authentication refusal.'),
        e('signature', 401, code('400005', 'Invalid KC-API-SIGN'), 'rejected', 'Gateway auth codes 400003-400007 prove nothing was placed.'),
        e('permission', 200, code('400007', 'The API key does not have permission to access this resource.'), 'rejected', 'A key without Unified is refused before placement.'),
        e('insufficient_funds', 200, code('116355', 'Insufficient funds.'), 'rejected', 'Documented UTA insufficient-funds code.'),
        e('throttle_user', 429, code('429000', 'Too many requests. User-level rate limit exceeded, please retry later.'), 'rejected', 'Throttle: nothing placed.'),
        e('throttle_system', 200, code('429001', 'Too many requests. System-level rate limit exceeded, please retry later.'), 'rejected', 'System throttle: nothing placed.'),
        e('throttle_http', 429, None, 'rejected', 'HTTP 429 without a body.'),
        e('safe_reschedule', 200, code('400002', 'Invalid KC-API-TIMESTAMP'), 'safe', 'Timestamp refusal keeps the classic safe-reschedule meaning.'),
        e('system_busy', 200, code('166115', 'System busy. Please try again later.'), 'ambiguous', 'Busy may or may not have placed.'),
        e('request_timeout', 200, code('111201', 'Request timeout.'), 'ambiguous', 'A timeout after send cannot be told.'),
        e('gateway_timeout', 504, code('504000', 'Gateway timeout.'), 'ambiguous', 'A gateway timeout cannot be told.'),
        e('server_error_no_body', 500, None, 'ambiguous', '5xx without a readable code.'),
        e('unknown_code', 200, code('999999', 'unexpected'), 'ambiguous', 'An undocumented code never proves a refusal.'),
        e('success_without_order_id', 200, ok({'tradeType': 'SPOT', 'clientOid': CLIENT_OID, 'ts': 1}), 'ambiguous', 'Success with no orderId.'),
        e('success_other_client_oid', 200, ok({'orderId': ORDER_ID, 'tradeType': 'SPOT', 'clientOid': 'someone-else', 'ts': 1}), 'ambiguous', 'Another clientOid cannot be ours.'),
        e('malformed_success', 200, '{"code":"200000","data":', 'ambiguous', 'Truncated 2xx body.'),
    ]}


def fills():
    items = [execution(str(1943106280000 + i), '0.0001', '5', fee='0.005', order_id=str(385577101236910000 + i // 4),
                       side='BUY' if i % 3 else 'SELL', at_ms=PLACED_MS + i * 1000) for i in range(203)]
    first, second = items[:200], items[199:]  # the 200th fill repeats at the next page's start
    walk = [page(first, LAST_ID), page(second, LAST_ID + 1), page([], LAST_ID + 1)]
    buys = [i for i in items if i['side'] == 'BUY']
    expect = {'rows': 203, 'buy_rows': len(buys), 'sell_rows': 203 - len(buys), 'cursors_sent': [None, str(LAST_ID), str(LAST_ID + 1)],
              'identities': 'symbol-tradeId', 'created_at_ms': 'executionTime / 10^6, exact'}
    q = [['tradeType', 'SPOT'], ['startAt', str(PLACED_MS)], ['endAt', str(PLACED_MS + 7 * 86400 * 1000 - 1)], ['pageSize', '200']]
    fail = {'rows_published_by_failed_page': 0, 'read_recorded': False}
    one = [execution('1943106281000', '0.0001', '5')]
    return {'cases': [
        case('walk_account_wide', ['execution'], 'Account-wide SPOT walk (symbol optional): 200 + 3 with a repeated boundary fill, returned lastId above 2^53, ends only on an empty page.',
             expect, request={'path': '/api/ua/v2/unified/order/execution', 'query': q}, pages=walk),
        case('walk_short_page_continues', ['execution'], 'A short page is not the end.', {'rows': 1, 'pages': 2},
             pages=[page(one, 21), page([], 21)]),
        case('cursor_repeated', ['execution'], 'A lastId equal to the one sent fails the window.', fail, pages=[page(one, 21), page(one, 21)]),
        case('cursor_missing', ['execution'], 'A non-empty page without lastId fails the window.', fail, pages=[ok({'tradeType': 'SPOT', 'items': one})]),
        case('cursor_not_integer', ['execution'], 'A lastId that is not an exact integer fails the window.', fail, pages=[page(one, '21')]),
        case('side_lowercase', ['execution'], 'Sides are exactly BUY or SELL (the classic mapper only knows lowercase buy).', fail, pages=[page([dict(one[0], side='buy')], 21)]),
        case('side_maps_to_buy', ['execution'], 'BUY lands as a buy row (positive holding), SELL as a sell row.', {'buy_rows': 1, 'sell_rows': 1},
             pages=[page([one[0], execution('1943106281001', '0.0001', '5', side='SELL')], 21), page([], 21)]),
        case('item_malformed', ['execution'], 'A malformed item fails the page.', fail, pages=[page([dict(one[0], size='-1')], 21)]),
        case('item_futures', ['execution'], 'A non-SPOT item fails the page.', fail, pages=[page([dict(one[0], tradeType='FUTURES')], 21)]),
        case('time_outside_window', ['execution'], 'An executionTime outside [startAt, endAt] fails the page.', fail,
             pages=[page([dict(one[0], executionTime=(PLACED_MS - 1) * 10**6)], 21)]),
        case('envelope_error', ['execution', 'errors'], 'A non-200000 envelope fails the window; it stays unread and retries after its delay.', fail,
             pages=[{'code': '166115', 'msg': 'System busy. Please try again later.'}]),
    ]}


def balances():
    def cur(c, equity, hold, balance, available, liability='0'):
        return {'currency': c, 'equity': equity, 'hold': hold, 'balance': balance, 'available': available, 'liability': liability, 'potentialBorrow': '0', 'collateralStatus': '1'}
    def body(*cs): return ok({'accountType': 'UNIFIED', 'ts': 1776089058432, 'accounts': [{'currencies': list(cs)}]})
    src = ['balance']
    return {'cases': [
        case('spot_plain', src, 'Holdings are balance; spendable is min(available, balance - hold).', {'held': {'BTC': '0.5', 'USDT': '1000'}, 'spendable': {'BTC': '0.4', 'USDT': '1000'}},
             request={'path': '/api/ua/v2/unified/account/balance', 'query': []},
             response=body(cur('BTC', '0.5', '0.1', '0.5', '0.4'), cur('USDT', '1000', '0', '1000', '1000'))),
        case('positive_unpnl', src, 'available = equity - hold includes positive unPnL; it is never spent.', {'held': {'USDT': '3052.9434208133'}, 'spendable': {'USDT': '3052.9434208133'}},
             response=body(cur('USDT', '3054.8568208133', '0.0000000000', '3052.9434208133', '3054.8568208133'))),
        case('negative_unpnl', src, 'A negative unPnL lowers available below balance - hold.', {'held': {'USDT': '100'}, 'spendable': {'USDT': '90'}},
             response=body(cur('USDT', '95', '5', '100', '90'))),
        case('liability', src, 'A currency with a liability keeps its previous row and is named; the others are stored (r6.2).',
             {'held': {'BTC': '0.5'}, 'spendable': {'BTC': '0.4'}, 'kept': ['USDT']},
             response=body(cur('BTC', '0.5', '0.1', '0.5', '0.4'), cur('USDT', '-10', '0', '-10', '0', liability='10'))),
        case('negative_balance', src, 'A currency with a negative balance keeps its previous row and is named; the others are stored (r6.2).',
             {'held': {'USDT': '10'}, 'spendable': {'USDT': '10'}, 'kept': ['BTC']},
             response=body(cur('BTC', '0', '0', '-0.001', '0'), cur('USDT', '10', '0', '10', '10'))),
        case('malformed', src, 'An unreadable amount refuses the read; previous rows stay.', {'refused': True},
             response=body(cur('USDT', '10', '0', 'x', '10'))),
    ]}


def apikey():
    def key(permission): return ok({'remark': 'UTA-0', 'apiKey': 'UTA0-EXAMPLE-KEY', 'apiVersion': 3, 'permission': permission, 'isMaster': True})
    src = ['apikey', 'introduction']
    return {'cases': [
        case('trading_unified', src, 'A UTA trading key needs Unified (place and cancel).', {'verdict': 'Correct', 'requests': 1}, kind=0, status=200, response=key('General,Unified')),
        case('trading_spot_only', src, 'Spot alone does not trade on UTA.', {'verdict': 'Permissions', 'missing': ['Unified']}, kind=0, status=200, response=key('General,Spot')),
        case('trading_token_exact', src, 'Tokens are compared exactly after trimming.', {'verdict': 'Permissions', 'missing': ['Unified']}, kind=0, status=200, response=key('General, Unified2,unified')),
        case('trading_trimmed', src, 'Surrounding spaces are trimmed.', {'verdict': 'Correct'}, kind=0, status=200, response=key('General, Unified ')),
        case('trading_error_status', src, 'A non-2xx answer is a failed check, even with a success-shaped body.', {'verdict': 'Failed'}, kind=0, status=500, response=key('General,Unified')),
        case('trading_no_permission_field', src, 'No permission string is a failed check.', {'verdict': 'Failed'}, kind=0, status=200, response=ok({'apiKey': 'UTA0-EXAMPLE-KEY'})),
        case('read_general', src, 'A read key needs General only.', {'verdict': 'Correct'}, kind=2, status=200, response=key('General')),
    ]}


def signing():
    cred = {'key': 'UTA0-EXAMPLE-KEY', 'secret': 'UTA0-example-secret', 'passphrase': 'UTA0 example passphrase'}
    ts = '1764741957000'
    def mac(text): return base64.b64encode(hmac.new(cred['secret'].encode(), text.encode(), hashlib.sha256).digest()).decode()
    def sign(cid, method, path, query=(), body=None, note='Throwaway credentials, frozen timestamp, coherent request bytes.'):
        raw = '&'.join(f'{k}={v}' for k, v in query)
        body_bytes = json.dumps(body, separators=(',', ':')) if body is not None else ''
        prehash = ts + method + path + (f'?{raw}' if raw else '') + body_bytes
        url = 'https://api.kucoin.com' + path + (f'?{urllib.parse.urlencode(list(query))}' if query else '')
        return case(cid, ['authentication', 'sdk_signer'], note,
                    {'prehash': prehash, 'transport_url': url,
                     'headers': {'KC-API-KEY': cred['key'], 'KC-API-TIMESTAMP': ts, 'KC-API-KEY-VERSION': '2', 'KC-API-SIGN': mac(prehash), 'KC-API-PASSPHRASE': mac(cred['passphrase'])}},
                    request={'method': method, 'path': path, 'query': [list(p) for p in query], 'body_bytes': body_bytes})
    place = {'tradeType': 'SPOT', 'clientOid': CLIENT_OID, 'symbol': SYMBOL, 'side': 'BUY', 'orderType': 'MARKET', 'size': '100.5', 'sizeUnit': 'QUOTECCY'}
    return {'credentials': cred, 'timestamp': ts, 'oracle': 'Python standard-library hmac/SHA256 + Base64, independent of Rust signer and Rails', 'cases': [
        sign('sign_get_balance', 'GET', '/api/ua/v2/unified/account/balance'),
        sign('sign_get_detail_order', 'GET', '/api/ua/v2/unified/order/detail', [('tradeType', 'SPOT'), ('symbol', SYMBOL), ('orderId', ORDER_ID)]),
        sign('sign_get_detail_client', 'GET', '/api/ua/v2/unified/order/detail', [('tradeType', 'SPOT'), ('symbol', SYMBOL), ('clientOid', CLIENT_OID)]),
        sign('sign_get_execution_cursor', 'GET', '/api/ua/v2/unified/order/execution',
             [('tradeType', 'SPOT'), ('startAt', str(PLACED_MS)), ('endAt', str(PLACED_MS + 604799999)), ('pageSize', '200'), ('lastId', str(LAST_ID))]),
        sign('sign_post_place', 'POST', '/api/ua/v2/unified/order/place', body=place),
        sign('sign_post_cancel', 'POST', '/api/ua/v2/unified/order/cancel', body={'tradeType': 'SPOT', 'symbol': SYMBOL, 'orderId': ORDER_ID}),
        sign('sign_get_unescaped_query', 'GET', '/api/ua/v2/unified/order/execution', [('tradeType', 'SPOT'), ('symbol', SYMBOL), ('remark', 'a b!@#')],
             note='Encoding probe: the prehash uses the un-encoded query, the URL is form-encoded; tests authentication only, not endpoint parameter acceptance.'),
    ]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--source-dir', required=True)
    ap.add_argument('--check', action='store_true')
    args = ap.parse_args()
    src = pathlib.Path(args.source_dir)
    sources, examples = {}, {}
    for sid, (name, url) in SOURCES.items():
        data = (src / name).read_bytes()
        sources[sid] = {'url': url, 'fetched_date': FETCHED, 'sha256': hashlib.sha256(data).hexdigest()}
        if (ex := example(data.decode())) is not None: examples[sid] = ex
    files = {'orders.json': {**orders(), 'source_examples': {k: examples[k] for k in ('place', 'cancel', 'detail') if k in examples}},
             'fills.json': {**fills(), 'source_examples': {k: examples[k] for k in ('execution',) if k in examples}},
             'balances.json': {**balances(), 'source_examples': {k: examples[k] for k in ('balance',) if k in examples}},
             'errors.json': errors(),
             'apikey.json': {**apikey(), 'source_examples': {k: examples[k] for k in ('apikey_ua',) if k in examples}},
             'signing.json': signing()}
    text = {n: json.dumps(v, indent=2, ensure_ascii=False) + '\n' for n, v in files.items()}
    endpoints = {
        'mode': {'method': 'GET', 'path': '/api/ua/v2/account/mode', 'source': 'mode'},
        'place': {'method': 'POST', 'path': '/api/ua/v2/unified/order/place', 'source': 'place'},
        'cancel': {'method': 'POST', 'path': '/api/ua/v2/unified/order/cancel', 'source': 'cancel'},
        'detail': {'method': 'GET', 'path': '/api/ua/v2/unified/order/detail', 'source': 'detail', 'required_query': ['tradeType', 'symbol'],
                   'retention_hours': {'cancelled_without_fills': 2, 'with_fills': 168}, 'clamps_silently': True},
        'execution': {'method': 'GET', 'path': '/api/ua/v2/unified/order/execution', 'source': 'execution', 'required_query': ['tradeType'],
                      'page_size_max': 200, 'default_range_days': 7, 'retention': 'UNKNOWN', 'max_range': 'UNKNOWN'},
        'balance': {'method': 'GET', 'path': '/api/ua/v2/unified/account/balance', 'source': 'balance'},
        'apikey': {'method': 'GET', 'path': '/api/v1/user/api-key', 'source': 'apikey', 'trading_permission': 'Unified', 'read_permission': 'General'},
    }
    manifest = {'schema_version': 1, 'oracle': 'official-docs-and-sdk; synthetic variants are not exchange recordings', 'sdk_revision': SDK,
                'sources': sources, 'recorder': 'script/rust/record_kucoin_uta.py',
                'recorder_sha256': hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),
                'generated_sha256': {n: hashlib.sha256(t.encode()).hexdigest() for n, t in text.items()},
                'endpoints': endpoints,
                'case_counts': {**{n[:-5]: len(files[n]['cases']) for n in files}, 'total': sum(len(f['cases']) for f in files.values())},
                'divergences': ['D-KUCOIN-UTA-ORDERS', 'D-KUCOIN-UTA-BALANCES', 'D-KUCOIN-UTA-LEDGER', 'D-KUCOIN-UTA-RETENTION', 'D-KUCOIN-CANCEL-OBLIGATION'],
                'unknown': ['classic and HF routes on a UTA account', 'pre-switch history through any route', 'execution retention and maximum range',
                            'funds location after a switch', 'UTA website export columns']}
    text['manifest.json'] = json.dumps(manifest, indent=2, ensure_ascii=False) + '\n'
    stale = [n for n, t in text.items() if not (OUT / n).exists() or (OUT / n).read_text() != t]
    if args.check:
        if stale: sys.exit(f'stale: {", ".join(stale)}')
        return
    OUT.mkdir(parents=True, exist_ok=True)
    for n in stale: (OUT / n).write_text(text[n])
    print(f'wrote {len(stale)} of {len(text)} files')


if __name__ == '__main__':
    main()
