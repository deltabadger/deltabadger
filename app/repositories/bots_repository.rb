class BotsRepository < BaseRepository
  SYMBOLS_HASH = {
    'XDG': 'DOGE',
    'XBT': 'BTC'
  }.freeze

  TOP_BOTS_KEY = 'TOP_BOTS_CACHE_KEY'.freeze

  def by_id_for_user(user, id)
    user.bots.without_deleted.find(id)
  end

  def by_webhook_for_user(webhook, user = nil)
    bots = user.present? ? user.bots : model
    bots.by_webhook(webhook)
  end

  def webhook_url
    loop do
      webhook = Array.new(8) { ('a'..'z').to_a.sample }.join
      break webhook unless model.by_webhook(webhook).present?
    end
  end

  def for_user(user, page_number)
    user
      .bots
      .without_deleted
      .includes(:exchange)
      .includes(:daily_transaction_aggregates)
      .includes(:transactions)
      .order(created_at: :desc)
      .page(page_number)
  end

  def count_with_status(status)
    model
      .where(status: status)
      .count
  end

  def model
    Bot
  end

  def top_ten_bots
    Rails.cache.exist?(TOP_BOTS_KEY) ? Rails.cache.read(TOP_BOTS_KEY) : top_bots_update
  end

  def top_bots_text(update = false)
    reply_text = "<b>Top 10 currencies by the number of bots:</b>\n"
    top_bots = update ? top_bots_update : top_ten_bots
    if top_bots.empty?
      return {
        reply_text: '<b>No bots are working at the moment.</b>',
        changed: false
      }
    end

    top_bots.each_with_index do |data, index|
      reply_text += "\n#{index + 1}. #{data[:name]} - #{data[:counter]} "
      reply_text += '⬆️' if data[:is_up]
    end
    changed = top_bots.find { |bot| bot[:is_up] }.present?
    {
      reply_text: reply_text,
      changed: changed
    }
  end

  def send_top_bots_update
    top_bots_text = top_bots_text(true)
    return unless should_send_update(top_bots_text[:changed])

    Telegram.bot.send_message(chat_id: ENV['TELEGRAM_GROUP_ID'],
                              text: top_bots_text[:reply_text],
                              parse_mode: 'html')
  end

  def top_bots_update
    top_ten_bots = most_popular_bots(10)
    top_bots_array = top_ten_bots.map do |key, value|
      {
        name: key,
        counter: value,
        is_up: false
      }
    end
    old_top_bots = Rails.cache.exist?(TOP_BOTS_KEY) ? Rails.cache.read(TOP_BOTS_KEY) : top_bots_array
    top_bots_array.each_with_index do |new_bot, new_index|
      old_index = old_top_bots.index { |bot| bot[:name] == new_bot[:name] }
      new_bot[:is_up] = up?(new_index, old_index)
    end
    Rails.cache.write(TOP_BOTS_KEY, top_bots_array, expires_in: 25.hour)
    top_bots_array
  end

  private

  def should_send_update(changed)
    Time.now.monday? || changed
  end

  def up?(new_index, old_index)
    old_index.nil? || new_index < old_index
  end

  def most_popular_bots(amount)
    search_hash = {
      status: 'working',
      bot_type: 'trading'
    }
    all_bots_hash = Bot.group("bots.settings->>'base'")
                       .order(count: :desc)
                       .where(search_hash)
                       .count
    fetched_symbols_bots_hash = all_bots_hash.each do |key, value|
      SYMBOLS_HASH.each do |symbol_key, symbol_value|
        if key == symbol_key.to_s
          all_bots_hash[symbol_value.to_s] += value
          all_bots_hash.delete(symbol_key.to_s)
        end
      end
    end
    fetched_symbols_bots_hash.sort_by { |_key, value| -value }[0..amount - 1]
  end
end
