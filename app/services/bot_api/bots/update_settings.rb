# frozen_string_literal: true

module BotApi
  module Bots
    # Updates the settings a stopped bot exposes to automation: amount and label on any bot, the
    # index knobs on an index bot, and the basket weights on a basket bot. Membership is not
    # editable here — only how much of each the bot buys.
    class UpdateSettings
      def self.call(user:, bot_id:, **attrs)
        new(user: user, bot_id: bot_id, **attrs).call
      end

      def initialize(user:, bot_id:, quote_amount: nil, label: nil, num_coins: nil,
                     allocation_flattening: nil, allocations: nil)
        @user = user
        @bot_id = bot_id
        @quote_amount = quote_amount
        @label = label
        @num_coins = num_coins
        @allocation_flattening = allocation_flattening
        @allocations = allocations
      end

      def call
        bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
        return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot

        if bot.working?
          return Result.failure(:conflict, 'bot_running',
                                "Bot must be stopped before updating settings. Current status: #{bot.status}.")
        end

        updates = build_updates(bot)
        return updates if updates.is_a?(Result)
        return Result.failure(:validation_failed, 'no_updates_provided', 'No settings provided to update.') if updates.empty?

        apply(bot, updates)

        if bot.save
          Result.success({ id: bot.id, label: bot.label, updated: updates.keys.map(&:to_s) })
        else
          Result.failure(:validation_failed, 'bot_save_failed',
                         "Failed to update bot: #{bot.errors.full_messages.join(', ')}")
        end
      end

      private

      # A Hash of accepted updates, or a Result carrying the first refusal.
      def build_updates(bot)
        updates = {}

        if @quote_amount.present?
          amount = Number.parse(@quote_amount)
          return invalid_number('quote_amount') unless amount&.positive?

          # Floats at the model boundary, as the model stores them.
          updates[:quote_amount] = amount.to_f
        end
        updates[:label] = @label if @label.present?

        if @num_coins.present? || @allocation_flattening.present?
          return unsupported('num_coins / allocation_flattening', 'index') unless bot.dca_index?

          # Strict: 'abc'.to_f is 0, which would silently flatten an index or fail validation
          # with a message about a number the caller never sent.
          if @num_coins.present?
            count = Number.integer(@num_coins)
            return invalid_number('num_coins') unless count

            updates[:num_coins] = count
          end
          if @allocation_flattening.present?
            flattening = Number.within(@allocation_flattening, 0.0..1.0)
            return invalid_number('allocation_flattening') unless flattening

            updates[:allocation_flattening] = flattening.to_f
          end
        end

        if @allocations.present?
          return unsupported('allocations', 'basket') unless bot.dca_multi_asset?

          weights = basket_weights(bot)
          return weights if weights.is_a?(Result)

          updates[:allocations] = weights
        end

        updates
      end

      def apply(bot, updates)
        updates.each do |key, value|
          if key == :allocations
            # Giving weights by hand is what the wizard's slider does: it takes the basket off
            # market-cap weighting.
            bot.assign_attributes(allocations: value, weighting: 'manual')
          else
            bot.public_send("#{key}=", value)
          end
        end
        bot.set_missed_quote_amount
      end

      def basket_weights(bot)
        given = parse_allocations(@allocations)
        unless given
          return Result.failure(:validation_failed, 'invalid_allocations',
                                "allocations must be 'SYMBOL:percent,…' or {symbol: percent}.")
        end

        by_symbol = bot.base_assets.index_by(&:symbol)
        if (stranger = given.keys.find { |symbol| !by_symbol.key?(symbol) })
          return Result.failure(:validation_failed, 'asset_not_in_basket',
                                "#{stranger} is not in this basket; membership cannot be changed here.")
        end
        if (missing = by_symbol.keys - given.keys).any?
          return Result.failure(:validation_failed, 'missing_basket_asset',
                                "Give a weight for every basket asset; missing: #{missing.join(', ')}.")
        end
        balanced = (given.values.sum - 100).abs <= 0.1
        return Result.failure(:validation_failed, 'allocations_unbalanced', 'Weights must sum to 100.') unless balanced

        # Floats keyed by asset id, as the composition stores them.
        given.to_h { |symbol, pct| [by_symbol[symbol].id.to_s, (pct / 100).to_f] }
      end

      # 'BTC:60,ETH:40' or a hash-shaped body; anything else — a bare array, a nested list, a
      # repeated symbol, a weight that is not a plain number — is nil, which the caller turns
      # into invalid_allocations rather than letting to_h raise into a 500.
      def parse_allocations(raw)
        pairs =
          if raw.is_a?(String)
            raw.split(',').map { |entry| entry.strip.split(':', 2) }
          elsif raw.is_a?(Hash) || raw.is_a?(ActionController::Parameters)
            raw.to_h.to_a
          end
        return nil if pairs.nil? || pairs.empty?
        return nil if pairs.any? { |symbol, pct| symbol.blank? || Number.within(pct, 0.0..100.0).nil? }

        symbols = pairs.map { |symbol, _| symbol.to_s.strip.upcase }
        return nil if symbols.uniq.size != symbols.size

        symbols.zip(pairs.map { |_, pct| Number.parse(pct) }).to_h
      end

      def unsupported(setting, kind)
        Result.failure(:validation_failed, 'unsupported_setting', "#{setting} applies to #{kind} bots only.")
      end

      def invalid_number(setting)
        Result.failure(:validation_failed, 'invalid_number', "#{setting} must be a number.")
      end
    end
  end
end
