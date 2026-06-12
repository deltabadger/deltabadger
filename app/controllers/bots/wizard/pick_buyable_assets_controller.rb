# Shared "pick the asset to buy" wizard step (single first step, dual second
# asset, signals first step). The three `new` actions genuinely diverge
# (session init vs prerequisite guard, label timing), so subclasses keep their
# own; the base holds the shared create skeleton and the view-state setup.
class Bots::Wizard::PickBuyableAssetsController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable
  include Bots::WizardSessionGuard

  def create
    if bot_params[asset_id_param].present?
      prepare_session_for_pick
      bot = build_bot
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
      redirect_after_asset_picked
    else
      prepare_step
      render :new, status: :unprocessable_entity
    end
  end

  private

  def bot_relation
    raise NotImplementedError
  end

  def build_bot = bot_relation.new(sanitized_bot_config)

  # Session adjustment before storing the pick (single restarts the wizard,
  # signals re-initialises an expired session, dual leaves it untouched).
  def prepare_session_for_pick; end

  # View state the :new template needs — shared by `new` and `create`'s 422 re-render.
  def prepare_step
    @bot = build_bot
    # Clear any previously-picked asset in memory so the list isn't filtered
    # against it — re-visiting the step should show the full set, including
    # the current pick.
    clear_picked_asset(@bot)
    @assets = asset_search_results(@bot, search_params[:query], :base_asset)
  end

  def search_params
    params.permit(:query)
  end
end
