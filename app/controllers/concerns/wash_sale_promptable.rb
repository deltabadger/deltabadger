# The wash-sale question, wherever it is asked. It is asked once per account, at the first action
# that can put a sell order on the book — not at bot start, which would put a tax question to people
# who only ever buy.
module WashSalePromptable
  extend ActiveSupport::Concern

  included do
    helper_method :wash_sale_prompt_due?, :wash_sale_prompt_stream
  end

  private

  def wash_sale_prompt_due?(bot)
    user_signed_in? && !current_user.wash_sale_decided? && bot.sell_capable?
  end

  # These actions answer with turbo_stream and never re-render the layout, so the modal has to be
  # part of the answer — a layout-level content_for would not fire until the user next navigated.
  # Returns nil when nothing is due, so callers can .compact it in beside their own streams.
  def wash_sale_prompt_stream(bot)
    return nil unless wash_sale_prompt_due?(bot)

    turbo_stream.replace('modal', partial: 'bots/wash_sale_prompts/dialog', locals: { bot: bot })
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
