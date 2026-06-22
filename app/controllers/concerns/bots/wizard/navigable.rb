# Order-derived wizard navigation for the single/dual bot-creation flow. Included
# ONLY by the DcaSingleAsset/DcaDualAsset step controllers — it leans on
# Bots::Wizard::StepOrder, which models only those two types. DcaIndex/Signals
# keep their hardcoded template hooks and never include this.
#
# The host controller supplies three things:
#   - current_step  : the abstract key this controller represents (:exchange, …)
#   - step_path(key): maps an abstract key to that controller's route helpers
#                     (knows the :currencies = single-picker quirk + stock bounce)
#   - finalise!     : terminal hand-off (only the spendable controller needs it)
# bot_relation is already defined on every step controller.
module Bots::Wizard::Navigable
  extend ActiveSupport::Concern

  included do
    helper_method :current_order, :current_variant if respond_to?(:helper_method)
  end

  private

  # Ephemeral wizard state: absent ⇒ asset_first, so every legacy session and
  # deep link keeps working. Only ever written via the POST order switch.
  def current_variant
    session.dig(:bot_config, 'flow').presence&.to_sym || :asset_first
  end

  def asset_first? = current_variant == :asset_first

  def current_bot_type
    @current_bot_type ||= begin
      bot = bot_relation.new
      case bot
      when Bots::DcaDualAsset then :dual
      when Bots::DcaSingleAsset then :single
      else
        raise "Bots::Wizard::Navigable cannot serve #{bot.class} — only single/dual bots have a StepOrder"
      end
    end
  end

  def current_order
    @current_order ||= Bots::Wizard::StepOrder.for(bot_type: current_bot_type, variant: current_variant)
  end

  # Move to the next step in the current order, or finalise when this is the last.
  def advance!
    nxt = current_order.next_after(current_step)
    nxt ? redirect_to(step_path(nxt)) : finalise!
  end

  # :api completeness lives in the DB (validated key), not the session, so it is
  # checked here rather than in the pure StepOrder object.
  def step_complete?(key)
    return @bot&.api_key&.correct? || false if key == :api

    current_order.owned_keys(key).all? { |path| session.dig(:bot_config, *path).present? }
  end

  def first_incomplete
    current_order.steps.find { |key| !step_complete?(key) } || current_order.steps.last
  end

  # Bounce back when an upstream step is unsatisfied (stale/direct URL). Replaces
  # the per-controller blank?/correct? guards with one order-aware rule.
  def prerequisite_redirect_path
    target = first_incomplete
    steps = current_order.steps
    step_path(target) if steps.index(target) < steps.index(current_step)
  end

  # Clear everything this step (and the steps after it) owns, so re-committing a
  # step invalidates downstream picks. The full-universe basis also drops stale
  # dual keys that would otherwise leak through sanitized_bot_config.
  def reset_downstream!
    session[:bot_config] ||= {}
    current_order.reset_keys(current_step).each { |path| delete_session_path(path) }
  end

  def delete_session_path(path)
    *parents, leaf = path
    container = parents.empty? ? session[:bot_config] : session.dig(:bot_config, *parents)
    container&.delete(leaf)
  end
end
