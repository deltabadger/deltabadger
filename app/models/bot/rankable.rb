module Bot::Rankable
  extend ActiveSupport::Concern

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
      all_bots_hash = combine_hashes(dca_single_asset_bots_hash, dca_dual_asset_bots_hash, dca_index_bots_hash)
      all_bots_hash.sort_by { |_, v| -v }[0...amount]
    end

    def dca_single_asset_bots_hash
      all_bots_hash = Bot.dca_single_asset
                         .working
                         .group("json_extract(bots.settings, '$.base_asset_id')")
                         .count
      all_bots_hash.transform_keys { |key| Asset.find(key).symbol }
    end

    def dca_dual_asset_bots_hash
      all_bots_hash0 = Bot.dca_dual_asset
                          .working
                          .group("json_extract(bots.settings, '$.base0_asset_id')")
                          .count
      all_bots_hash1 = Bot.dca_dual_asset
                          .working
                          .group("json_extract(bots.settings, '$.base1_asset_id')")
                          .count
      all_bots_hash = combine_hashes(all_bots_hash0, all_bots_hash1)
      all_bots_hash.transform_keys { |key| Asset.find(key).symbol }
    end

    def dca_index_bots_hash
      all_bots_hash = BotIndexAsset.joins(:bot)
                                   .where(bots: { type: 'Bots::DcaIndex', status: Bot.statuses.values_at(:scheduled, :executing, :retrying, :waiting) })
                                   .group(:asset_id)
                                   .count
      all_bots_hash.transform_keys { |key| Asset.find(key).symbol }
    end

    def combine_hashes(*hashes)
      hashes.each_with_object(Hash.new(0)) do |h, result|
        h.each { |k, v| result[k] += v }
      end
    end
  end
end
