# Shared "pick the exchange" wizard step. Subclasses supply the bot relation,
# routes and params as explicit overrides; the index step overrides `new`,
# the view state and the search wholesale (market-data/index gates, label
# init, per-exchange coin previews) and shares only the create flow.
class Bots::Wizard::PickExchangesController < ApplicationController
  before_action :authenticate_user!
  before_action :redirect_if_session_expired, only: :create

  include Bots::Searchable
  include Bots::WizardSessionGuard

  def new
    @bot = build_bot

    if (path = prerequisite_redirect_path)
      redirect_to path
    else
      prepare_step
    end
  end

  def create
    if bot_params[:exchange_id].present?
      session[:bot_config].merge!({ exchange_id: bot_params[:exchange_id] }.stringify_keys)
      redirect_to add_api_key_path
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

  # An earlier-step prerequisite is missing (stale/direct URL) — subclasses
  # return a path to bounce back to; nil means proceed.
  def prerequisite_redirect_path = nil

  # View state the :new template needs — shared by `new` and `create`'s 422 re-render.
  def prepare_step
    @bot = build_bot
    @bot.exchange_id = nil
    @exchanges = exchange_search_results(@bot, search_params[:query])
  end

  def search_params
    params.permit(:query)
  end
end
