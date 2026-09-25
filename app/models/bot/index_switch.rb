# The bot menu's index entries: a portfolio starts following an index ("Follow index"), an index bot
# changes its index ("Change the index") or keeps its members at their weights as a portfolio
# ("Custom allocation").
#
# Portfolio and index bots are one composition stack (Bot::Composition::*): membership lives in
# bot_index_assets and every figure is derived by walking the transactions. So the row switches class
# in place — same id, history, schedule and memberships — as Bot::SingleToComposition does. Held assets
# the new composition leaves out become exited members, the table with the Sell button.
#
# Only a stopped bot switches, with no swap, sale or redeploy in flight, and only under the venue's
# trading lease (Bot::VenueLease): stopped bots still rebalance (Bot::EvaluateRebalancersJob), and the
# lease is what keeps any trading job from running against the composition being replaced. What the
# lease does not cover — a stale instance saving, starting, or writing members it derived before the
# commit — is refused where those writes happen (Bot#refuse_stale_type, Lifecycle#start,
# Composition::Allocatable#update_bot_index_assets).
module Bot::IndexSwitch
  class Refused < StandardError; end

  module_function

  # @return [String, nil] why this bot cannot switch now, or nil
  def refusal(bot)
    return t(:stop_first) unless bot.stopped? || bot.created?
    return t(:in_flight) if bot.rebalance_pending? || bot.liquidation_in_flight? || bot.redeploy_in_flight?
    return t(:in_flight) if poll_in_flight?(bot)
    # An index bot cannot sell: a selling portfolio would silently start buying.
    return t(:selling) if bot.try(:selling?)

    nil
  end

  # A portfolio or an index bot starts following `index`, with a new bot's defaults for its size.
  # @raise [Refused]
  # @return [Bots::DcaIndex]
  def follow!(bot, index)
    switched = switch!(bot, Bots::DcaIndex) do |_source, target|
      target.assign_attributes(index.bot_settings)
      size = target.bounded_universe_size
      target.num_coins = size || 10
      target.hold_all = size.present?
      target.allocation_flattening ||= 0.0
    end
    # Derived now, so the page the user lands on already shows the new members and what left. The
    # resync the save queued stays as the fallback when the market-data service does not answer.
    switched.refresh_composition
    switched
  end

  # An index bot becomes a portfolio of its current members at their current weights, rounded to the
  # slider's 0.1%. A member its venue no longer trades at the quote is left out, and shows as exited.
  # @raise [Refused]
  # @return [Bots::DcaMultiAsset]
  def customize!(bot)
    # The stored weights follow the settings through a background resync; read them fresh, so a
    # flattening slider moved a moment ago is what the portfolio keeps. Outside the lease: deriving
    # asks the market-data service, and a failure leaves the stored weights to be used as they are.
    bot.refresh_composition if bot.dca_index?
    switch!(bot, Bots::DcaMultiAsset) do |source, target|
      tradeable = Ticker.available.trading_enabled
                        .where(exchange_id: source.exchange_id, quote_asset_id: source.quote_asset_id)
                        .select(:base_asset_id)
      weights = source.bot_index_assets.in_index.where(asset_id: tradeable)
                      .order(target_allocation: :desc).pluck(:asset_id, :target_allocation)
                      .to_h { |asset_id, weight| [asset_id.to_s, weight.to_f] }
      raise Refused, t(:no_members) if weights.empty?

      target.allocations = target.normalize_allocations(weights)
    end
  end

  def switch!(bot, to)
    reason = refusal(bot)
    raise Refused, reason if reason

    from = bot.class
    switched = nil
    held = Bot::VenueLease.hold([bot.exchange], holder: "Bot::IndexSwitch for bot #{bot.id}") do
      Bot.transaction do
        # Re-read inside the transaction (BEGIN IMMEDIATE): a Start, a merge or another switch that
        # committed since the request loaded the bot is seen here. STI-scoped, so a row that became
        # another class reads as gone.
        source = from.find_by(id: bot.id)
        raise Refused, t(:gone) if source.nil? || source.deleted? || source.archived?

        # The lease is the exchange the request saw, and the index was offered for that exchange and
        # currency: a bot moved since trades under a lease we do not hold.
        moved = source.exchange_id != bot.exchange_id || source.quote_asset_id.to_i != bot.quote_asset_id.to_i
        raise Refused, t(:gone) if moved

        reason = refusal(source)
        raise Refused, reason if reason

        renamed = source.label != source.default_label
        # The carry the source class computes, capped as on any settings edit (Bot::Accountable).
        source.set_missed_quote_amount
        target = from == to ? source : source.becomes!(to)
        # Only the target's own settings survive: an index bot has no conditions or sell direction,
        # and a portfolio keeps no index. Shared keys (amount, interval, smart interval, limit
        # orders, rebalancing) carry over, and becomes! has run the target's after_initialize, so
        # its own defaults are there too.
        target.settings = target.settings.slice(*to.stored_attributes[:settings].map(&:to_s))
        yield source, target
        # A name the user never changed follows what the bot now holds.
        target.label = nil unless renamed
        target.save!
        switched = target
      end
      next if from == to

      # Still under the lease. The portfolio's condition polls first: an index bot has none of those
      # conditions, and a poll repointed to it would call methods it lacks once it runs.
      bot.send(:cancel_scheduled_limit_check_jobs) if bot.respond_to?(:cancel_scheduled_limit_check_jobs, true)
      Bot::ConversionQueue.repoint(bot.id, from: from.name, to: to.name)
    end
    raise Refused, t(:in_flight) unless held

    Bot.find(switched.id)
  rescue ActiveRecord::RecordInvalid => e
    raise Refused, e.record.errors.full_messages.to_sentence
  end

  # A portfolio's condition poll that is running, ready, blocked or due. Polls run outside the venue
  # lease, and one that finishes after the switch would re-arm under the old class. Only polls: the
  # page's own broadcasts name the bot too, and would refuse a switch clicked right after it loaded.
  def poll_in_flight?(bot)
    return false unless defined?(SolidQueue)

    polls = SolidQueue::Job.where(class_name: Bot::LimitCheckable::LIMIT_CHECK_JOBS.values.map { |job, _| job.name })
                           .where('arguments LIKE ?', "%#{bot.class.name}/#{bot.id}\"%").select(:id)
    Bot::ConversionQueue::BUSY_EXECUTIONS.any? { |name| SolidQueue.const_get(name).exists?(job_id: polls) } ||
      SolidQueue::ScheduledExecution.due.exists?(job_id: polls)
  end

  def t(key) = I18n.t("errors.bots.index_switch.#{key}")
end
