module Bot::Rankable
  extend ActiveSupport::Concern

  SYMBOLS_HASH = {
    'XDG': 'DOGE',
    'XBT': 'BTC'
  }.freeze
  TOP_BOTS_KEY = 'TOP_BOTS_CACHE_KEY'.freeze

  class_methods do
    def top_bots_text(update: false)
      top_bots = if Rails.cache.exist?(TOP_BOTS_KEY) && !update
                   Rails.cache.read(TOP_BOTS_KEY)
                 else
                   get_top_bots
                 end

      if top_bots.empty?
        {
          reply_text: '<b>No bots are working at the moment.</b>',
          changed: false
        }
      else
        reply_text = "<b>Top 10 currencies by the number of bots:</b>\n"
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
    end

    private

    def get_top_bots
      top_bots_array = most_popular_bots(10).map do |key, value|
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

    def up?(new_index, old_index)
      old_index.nil? || new_index < old_index
    end

    def most_popular_bots(amount)
      search_hash = {
        status: 'working',
        type: 'Bots::Basic'
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
end
