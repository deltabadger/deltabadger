# The wash-sale question, wherever it is asked. It is asked once per account, at the first action
# that can put a sell order on the book — not at bot start, which would put a tax question to people
# who only ever buy.
module WashSalePromptable
  extend ActiveSupport::Concern

  included do
    helper_method :wash_sale_prompt_due?, :wash_sale_selling_warning_due?, :wash_sale_prompt_stream
  end

  private

  def wash_sale_prompt_due?(bot)
    user_signed_in? && !current_user.wash_sale_decided? && bot.sell_capable?
  end

  # What protection cannot cover while a multi-asset bot sells: it stops Deltabadger buying an asset
  # back after a loss sale, but it does not check what was bought BEFORE one — and a selling basket
  # sells a bit at a time. Told once per account, to accounts that apply the rule, when a basket is set
  # (or armed through a "start selling" condition) to sell.
  #
  # Judged on the SAVED bot: a settings save that failed re-renders with the rejected attributes still
  # assigned, and a warning acknowledged for an arming that never saved would silence the real one. Re-
  # read rather than refused when changed — a freshly loaded bot can read as changed too, from the
  # settings defaults after_initialize fills in (Bot::Lifecycle), and must still be warned.
  # Kept apart from wash_sale_prompt_due?, which Bots::StartsController uses as a gate on starting.
  def wash_sale_selling_warning_due?(bot)
    return false unless user_signed_in? && current_user.wash_sale_enabled? &&
                        current_user.wash_sale_selling_warned_at.nil? && bot.dca_multi_asset? && bot.persisted?

    saved = bot.changed? ? bot.class.find(bot.id) : bot
    saved.selling? || saved.armed_to_start_selling?
  end

  # These actions answer with turbo_stream and never re-render the layout, so the modal has to be
  # part of the answer — a layout-level content_for would not fire until the user next navigated.
  # The question first; the selling warning only once the question is answered. Returns nil when
  # nothing is due, so callers can .compact it in beside their own streams.
  def wash_sale_prompt_stream(bot)
    partial = if wash_sale_prompt_due?(bot)
                'bots/wash_sale_prompts/dialog'
              elsif wash_sale_selling_warning_due?(bot)
                'bots/wash_sale_prompts/selling_warning'
              end
    return nil unless partial

    turbo_stream.replace('modal', partial:, locals: { bot: })
  end

  # Record the answer as the question partial submits it — the same shape from the modal, the Sell
  # confirmation and the account box, because they render the same partial. nil when neither choice
  # was picked, so a caller can refuse rather than record a "no" nobody made; false when the answer
  # itself was rejected.
  def record_wash_sale_answer
    answer = params.dig(:wash_sale, :enabled)
    return nil if answer.blank?

    record_wash_sale_decision(enabled: ActiveModel::Type::Boolean.new.cast(answer),
                              jurisdiction: params.dig(:wash_sale, :jurisdiction))
  end

  # One writer for the decision, wherever it is answered: the account box, the modal, or the Sell
  # confirmation. Switching it on re-walks the account ledger, so a window that started before the
  # answer is still served. Returns false on an unknown jurisdiction, so the caller can refuse.
  def record_wash_sale_decision(enabled:, jurisdiction: nil)
    attributes = { wash_sale_enabled: enabled }
    attributes[:wash_sale_jurisdiction] = jurisdiction if jurisdiction.present?
    was_enabled = current_user.wash_sale_enabled?

    current_user.update(attributes).tap do |saved|
      Tracker::LedgerJob.perform_later(current_user.id) if saved && current_user.wash_sale_enabled? && !was_enabled
    end
  end
end
