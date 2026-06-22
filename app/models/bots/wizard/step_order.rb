# Pure value object owning the bot-creation wizard's STEP ORDER ONLY — no DB, no
# routes. It sequences the existing step keys (:currencies, :currencies2,
# :exchange, :api, :spendable — the same symbols the views/i18n already use) per
# (bot_type, variant), and derives three things from that sequence: progress %,
# the session key(s) each step owns, and the downstream reset set.
#
# Everything genuinely irregular (route resolution, the conditional stock-broker
# step, single→dual promotion, API-key completeness) stays in the controllers /
# Navigable concern — this object deliberately does not model it.
class Bots::Wizard::StepOrder
  # Session key paths (relative to session[:bot_config]). exchange_id is
  # top-level; the asset ids live under "settings".
  EXCHANGE_KEY = %w[exchange_id].freeze
  BASE_KEY     = %w[settings base_asset_id].freeze
  BASE0_KEY    = %w[settings base0_asset_id].freeze
  BASE1_KEY    = %w[settings base1_asset_id].freeze
  QUOTE_KEY    = %w[settings quote_asset_id].freeze

  # The full universe of wizard-owned session keys. reset_keys subtracts from
  # this, so it always covers the stale dual keys (base0/base1) even in a single
  # flow — preventing them from leaking through sanitized_bot_config.
  ALL_WIZARD_KEYS = [EXCHANGE_KEY, BASE_KEY, BASE0_KEY, BASE1_KEY, QUOTE_KEY].freeze

  SEQUENCES = {
    single: {
      asset_first: %i[currencies exchange api spendable],
      exchange_first: %i[exchange api currencies spendable]
    },
    dual: {
      asset_first: %i[currencies currencies2 exchange api spendable],
      exchange_first: %i[exchange api currencies currencies2 spendable]
    }
  }.freeze

  def self.for(bot_type:, variant: :asset_first)
    new(bot_type: bot_type.to_sym, variant: variant.to_sym)
  end

  attr_reader :bot_type, :variant, :steps

  def initialize(bot_type:, variant:)
    @bot_type = bot_type
    @variant = variant
    @steps = SEQUENCES.fetch(bot_type).fetch(variant)
  end

  def first = @steps.first

  def first?(step) = step == first

  def next_after(step)
    index = @steps.index(step)
    return nil if index.nil? || index == @steps.size - 1

    @steps[index + 1]
  end

  # Index-based even spacing: a 4-step flow yields 25/50/75/100, a 5-step flow
  # 20/40/60/80/100 — automatically correct for either variant.
  def progress(step)
    index = @steps.index(step)
    return 0 if index.nil?

    ((index + 1) * 100.0 / @steps.size).round
  end

  # The session key(s) a step writes. :currencies is base0 in a dual flow,
  # base_asset_id in a single one; :api lives in the DB, so it owns nothing here.
  def owned_keys(step)
    case step
    when :currencies  then [bot_type == :dual ? BASE0_KEY : BASE_KEY]
    when :currencies2 then [BASE1_KEY]
    when :exchange    then [EXCHANGE_KEY]
    when :spendable   then [QUOTE_KEY]
    else []
    end
  end

  ASSET_STEPS = %i[currencies currencies2].freeze

  # Keys to clear when (re-)committing a step: the whole universe minus whatever
  # is preserved. The steps BEFORE it are preserved (re-picking the first step is
  # a full wipe; re-picking a later step keeps the upstream answers).
  #
  # The chosen asset is additionally STICKY: only an asset step clears it. So
  # re-picking the exchange keeps the asset (the exchange list is asset-filtered,
  # so the asset stays valid) and just drops the pair-specific quote — in either
  # variant, not only asset-first.
  def reset_keys(step)
    index = @steps.index(step)
    earlier = index ? @steps[0...index] : []
    preserved = earlier.flat_map { |s| owned_keys(s) }
    preserved += asset_keys unless ASSET_STEPS.include?(step)
    ALL_WIZARD_KEYS - preserved
  end

  # The asset keys this flow actually uses (base for single, base0/base1 for dual).
  def asset_keys
    (@steps & ASSET_STEPS).flat_map { |s| owned_keys(s) }
  end
end
