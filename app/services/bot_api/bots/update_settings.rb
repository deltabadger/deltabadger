# frozen_string_literal: true

module BotApi
  module Bots
    # Changes a stopped bot's settings the way its settings page does: the page's field names and
    # choices (SettingsFields), saved through the page's own save (Bot::SettingsForm). What this adds is
    # the automation dialect only — strict numbers, true/false for the page's switches, null for a
    # cleared field, basket weights by symbol — turned into exactly what the page would post.
    class UpdateSettings
      def self.call(user:, bot_id:, **settings)
        new(user: user, bot_id: bot_id, **settings).call
      end

      # `settings` holds only what the caller sent: an omitted field is absent, a cleared one is nil.
      def initialize(user:, bot_id:, **settings)
        @user = user
        @bot_id = bot_id
        @settings = settings.transform_keys(&:to_s).slice(*SettingsFields::NAMES)
      end

      def call
        bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
        return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot

        # Settings are one JSON blob, written whole. Re-read the row under the write lock (SQLite
        # opens this as BEGIN IMMEDIATE) and change only this call's keys on it, so an edit that
        # committed after the load above — the user's new amount — is kept, not overwritten.
        bot.class.transaction { update(bot.class.lock.find(bot.id)) }
      end

      private

      def update(bot)
        if bot.working?
          return Result.failure(:conflict, 'bot_running',
                                "Bot must be stopped before updating settings. Current status: #{bot.status}.")
        end

        form = build_form(bot)
        return form if form.is_a?(Result)
        return Result.failure(:validation_failed, 'no_updates_provided', 'No settings provided to update.') if form.empty?

        if Bot::SettingsForm.save(bot, form)
          Result.success({ id: bot.id, label: bot.label, updated: SettingsFields::NAMES & form.keys })
        else
          Result.failure(:validation_failed, 'bot_save_failed',
                         "Failed to update bot: #{bot.errors.full_messages.join(', ')}")
        end
      end

      # What the settings page would post, or a Result carrying the first refusal. Checked in catalogue
      # order, so the original tool's refusals come first, as they always did.
      def build_form(bot)
        kind = SettingsFields.kind(bot)
        form = ActiveSupport::HashWithIndifferentAccess.new

        SettingsFields::FIELDS.each do |field|
          name = field.name
          next unless @settings.key?(name)

          value = @settings[name]
          if blank?(value)
            # The page posts a blank for a field it cleared; other fields ignore one.
            form[name] = nil if field.nullable
            next
          end
          unless field.kinds.include?(kind)
            # The original tool refused its two index knobs together, and still says so.
            setting = name.in?(%w[num_coins allocation_flattening]) ? 'num_coins / allocation_flattening' : name
            return unsupported(setting, field.applies)
          end

          posted = posted_value(bot, field, value)
          return posted if posted.is_a?(Result)

          form[name] = posted
        end
        form
      end

      # Absent, null, or blank text: the original tool treated these as "not given", and so does this.
      def blank?(value) = value.nil? || (value.is_a?(String) && value.strip.empty?)

      # The string the page would post for this value, or a Result refusal.
      def posted_value(bot, field, value)
        case field.name
        when 'label' then value
        when 'allocations' then basket_weights(bot)
        when 'quote_amount'
          amount = Number.parse(value)
          # Strict: 'abc'.to_f is 0, which would fail validation with a message about a number the
          # caller never sent.
          amount&.positive? ? amount.to_s('F') : invalid_number('quote_amount')
        when 'allocation_flattening'
          Number.within(value, 0.0..1.0)&.to_s('F') || invalid_number('allocation_flattening')
        else
          case field.type
          when 'boolean' then switch(value)
          when 'number' then number(field.name, value)
          else value.to_s
          end
        end
      end

      # A page switch posts '1' or '0'. A JSON false must become '0': the page's parser drops a blank, and
      # false is blank in Ruby, so it could never switch anything off.
      def switch(value)
        case value
        when true then '1'
        when false then '0'
        else value.to_s
        end
      end

      def number(name, value)
        if name.end_with?('_id', '_period', 'num_coins')
          Number.integer(value)&.to_s || invalid_number(name)
        else
          Number.parse(value)&.to_s('F') || invalid_number(name)
        end
      end

      def basket_weights(bot)
        given = parse_allocations(@settings['allocations'])
        unless given
          return Result.failure(:validation_failed, 'invalid_allocations',
                                "allocations must be 'SYMBOL:percent,…' or {symbol: percent}.")
        end

        # Every identifier to its asset first; a member is named once.
        keys = bot.member_keys
        weights = {}
        given.each do |identifier, pct|
          ids = member_ids_named(bot, keys, identifier)
          if ids.empty?
            return Result.failure(:validation_failed, 'asset_not_in_basket',
                                  "#{identifier} is not in this basket; add it with add_asset_id first.")
          end
          if ids.many?
            return Result.failure(:validation_failed, 'ambiguous_basket_asset',
                                  "#{identifier} names more than one basket asset: #{keys.values_at(*ids).join(', ')}. " \
                                  'Use one of those.')
          end
          return Result.failure(:validation_failed, 'invalid_allocations', "#{keys[ids.first]} is given twice.") if weights.key?(ids.first)

          weights[ids.first] = pct
        end
        # The page posts every member's slider: the ones the caller named at their new weight, the rest at
        # the weight the slider shows (rounded to 0.1). Balance is judged at start, as on the page.
        bot.allocations.keys.to_h do |asset_id|
          pct = weights[asset_id.to_i]
          [asset_id, pct ? pct.to_s('F') : (bot.allocation_for(asset_id) * 100).round(1).to_s]
        end
      end

      # The members an identifier names: the one with that key and the one with that asset id — both, when a
      # numeric key and an id name different members — else every member with that symbol (case-insensitive).
      def member_ids_named(bot, keys, identifier)
        by_id = identifier.to_i if identifier.match?(/\A\d+\z/) && keys.key?(identifier.to_i)
        exact = [keys.key(identifier), by_id].compact.uniq
        return exact if exact.any?

        bot.base_assets.select { |asset| asset.symbol&.casecmp?(identifier) }.map(&:id)
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

        identifiers = pairs.map { |identifier, _| identifier.to_s.strip }
        return nil if identifiers.map(&:upcase).uniq.size != identifiers.size

        identifiers.zip(pairs.map { |_, pct| Number.parse(pct) }).to_h
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
