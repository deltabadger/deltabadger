require 'digest'

# Thin API surface over the IBKR Web API used by Exchanges::Ibkr.
#
# Every brokerage call runs inside IbkrLock keyed by a fingerprint of the consumer key (= the
# single IBKR brokerage session), so concurrent jobs/web/MCP calls for one login are serialized.
# All calls return a Result; the underlying Clients::Ibkr::Session does the OAuth signing,
# session establishment, and one-shot self-heal.
class Clients::Ibkr < Client
  API = '/v1/api'.freeze

  REPLY_LIMIT = 5 # bound on the order confirmation ("are you sure?") loop

  def initialize(api_key:, session: nil)
    super()
    @api_key = api_key
    @session = session || Clients::Ibkr::Session.new(api_key: api_key)
  end

  # GET the accounts available to this login (account discovery).
  def accounts
    locked { @session.signed_request(:get, "#{API}/iserver/accounts") }
  end

  # Resolve a symbol (optionally constrained to a currency) to an IBKR conid.
  def search_contract(symbol:, currency: nil, sec_type: 'STK')
    locked do
      results = Array(@session.signed_request(:get, "#{API}/iserver/secdef/search", query: { symbol: symbol }))
      conid = pick_conid(results, currency: currency, sec_type: sec_type)
      conid ? Result::Success.new(conid) : Result::Failure.new("No #{sec_type} conid for #{symbol} #{currency}".strip)
    end
  end

  # Place an order and resolve any confirmation prompts. quantity is whole shares.
  def place_order(account_id:, conid:, side:, quantity:, order_type: 'MKT', price: nil, tif: 'DAY')
    order = { conid: conid.to_i, orderType: order_type, side: side.to_s.upcase, quantity: quantity, tif: tif }
    order[:price] = price if price
    locked do
      body = @session.signed_request(:post, "#{API}/iserver/account/#{account_id}/orders", body: { orders: [order] })
      Result::Success.new(resolve_replies(body))
    end
  end

  def order_status(order_id:)
    locked { @session.signed_request(:get, "#{API}/iserver/account/order/status/#{order_id}") }
  end

  def cancel_order(account_id:, order_id:)
    locked { @session.signed_request(:delete, "#{API}/iserver/account/#{account_id}/order/#{order_id}") }
  end

  def ledger(account_id:)
    locked { @session.signed_request(:get, "#{API}/portfolio/#{account_id}/ledger") }
  end

  def positions(account_id:, page: 0)
    locked { @session.signed_request(:get, "#{API}/portfolio/#{account_id}/positions/#{page}") }
  end

  # Market-data snapshot. IBKR often needs a "pre-flight" call before it returns fields, so the
  # caller (Exchanges::Ibkr) re-tries on an empty result.
  def snapshot(conids:, fields:)
    query = { conids: Array(conids).join(','), fields: Array(fields).join(',') }
    locked { @session.signed_request(:get, "#{API}/iserver/marketdata/snapshot", query: query) }
  end

  private

  # Wraps the block in the per-session lock + Result/error handling. If the block already
  # returns a Result it is passed through; otherwise its value is wrapped in Result::Success.
  def locked
    with_rescue do
      IbkrLock.with_lock(lock_key) do
        value = yield
        value.is_a?(Result) ? value : Result::Success.new(value)
      end
    end
  end

  def lock_key
    "ibkr:#{Digest::SHA256.hexdigest(@api_key.key.to_s)}"
  end

  # IBKR returns an array; an element with both `id` and `message` is a confirmation prompt that
  # must be answered via /iserver/reply/{id}. Loop (bounded) until no prompts remain.
  def resolve_replies(body)
    REPLY_LIMIT.times do
      arr = Array(body)
      prompt = arr.find { |e| e.is_a?(Hash) && e['id'].present? && e['message'].present? }
      return arr unless prompt

      body = @session.signed_request(:post, "#{API}/iserver/reply/#{prompt['id']}", body: { confirmed: true })
    end
    Array(body)
  end

  def pick_conid(results, currency:, sec_type:)
    candidates = results.select { |r| r.is_a?(Hash) && r['conid'].present? }
    candidates = candidates.select { |r| matches_sec_type?(r, sec_type) }
    chosen = if currency.present?
               candidates.find { |r| section_currency(r) == currency.to_s.upcase } || candidates.first
             else
               candidates.first
             end
    chosen && chosen['conid'].to_i
  end

  def matches_sec_type?(row, sec_type)
    sections = Array(row['sections'])
    sections.empty? || sections.any? { |s| s.is_a?(Hash) && s['secType'].to_s.casecmp?(sec_type) }
  end

  def section_currency(row)
    currency = row['currency'].presence || Array(row['sections']).filter_map { |s| s['currency'] }.first
    currency.to_s.upcase
  end

  # Surface IBKR's JSON error message (not the whole body) so error classification can match it.
  def with_rescue
    yield
  rescue *Client::TRANSIENT_NETWORK_ERRORS => e
    raise Client::TransientNetworkError, "#{e.class}: #{e.message}"
  rescue Faraday::Error => e
    Result::Failure.new(extract_ibkr_message(e.response_body) || "HTTP #{e.response_status || 'error'}",
                        data: { status: e.response_status })
  rescue IbkrLock::Timeout => e
    Result::Failure.new(e.message)
  rescue StandardError => e
    Result::Failure.new(e.message.presence || 'Unknown IBKR error')
  end

  def extract_ibkr_message(body)
    parsed = if body.is_a?(Hash)
               body
             else
               begin
                 JSON.parse(body)
               rescue StandardError
                 nil
               end
             end
    return nil unless parsed.is_a?(Hash)

    parsed['error'].presence || parsed['message'].presence || parsed.dig('error', 'message')
  end
end
