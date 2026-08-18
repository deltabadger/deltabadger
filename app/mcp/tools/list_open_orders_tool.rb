# frozen_string_literal: true

class ListOpenOrdersTool < ApplicationMCPTool
  tool_name 'list_open_orders'
  description 'List currently open (unfilled) orders across all connected exchanges'
  read_only

  property :exchange_name, type: 'string', description: 'Filter by exchange name (optional)'

  def perform
    result = BotApi::Orders::ListOpen.call(user: current_user, exchange_name: exchange_name)
    return render(text: result.error_message) unless result.success?

    if result.data[:count].zero? && result.data[:unavailable].blank?
      render text: 'No open orders found.'
      return
    end

    render text: present(result.data)
  end

  private

  def present(data)
    lines = data[:orders].map do |order|
      order[:source] == 'db' ? db_line(order) : exchange_line(order)
    end
    header = data[:count].zero? ? 'No open orders found on the exchanges that answered.' : "Open orders (#{data[:count]}):"
    ([header] + lines + unavailable_lines(data)).join("\n")
  end

  # Never silently. An exchange we could not ask is not an exchange with nothing on it.
  def unavailable_lines(data)
    Array(data[:unavailable]).map do |entry|
      "! #{entry[:exchange]}: could not be checked (#{entry[:error]}) — open orders there are not listed"
    end
  end

  def db_line(order)
    date = order[:created_at].strftime('%Y-%m-%d %H:%M')
    amount_str = order[:amount] ? order[:amount].to_s : 'N/A'
    price_str = order[:price] ? "@ #{order[:price]}" : ''
    type = order[:order_type]&.humanize || 'Unknown'
    "- [#{date}] ##{order[:id]} #{order[:side].upcase} #{amount_str} #{order[:pair]} " \
      "#{price_str} (#{type}) | #{order[:exchange]} | ext: #{order[:external_id]}"
  end

  def exchange_line(order)
    amount_str = order[:amount] ? order[:amount].to_s : 'N/A'
    price_str = order[:price] ? "@ #{order[:price]}" : ''
    type = order[:order_type] == 'limit_order' ? 'Limit order' : 'Market order'
    pair = order[:pair] || order[:external_id]
    side = order[:side]&.upcase || '?'
    "- #{side} #{amount_str} #{pair} #{price_str} (#{type}) | #{order[:exchange]} | ext: #{order[:external_id]}"
  end
end
