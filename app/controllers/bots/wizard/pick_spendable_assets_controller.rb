# Shared "pick the spendable (quote) asset" wizard step. For single/dual/index
# this is the final step: it applies the wizard defaults and creates the bot in
# its initial :created state, then breaks out of the modal_content Turbo frame
# to the bot's show page where the user fine-tunes settings. Signals overrides
# the post-pick hand-off (its wizard continues to confirm_settings). Subclasses
# supply the bot relation, routes, params and defaults as explicit overrides.
class Bots::Wizard::PickSpendableAssetsController < ApplicationController
  before_action :authenticate_user!
  before_action :redirect_if_session_expired, only: :create

  include Bots::Searchable

  def new
    @bot = build_bot
    @api_key = @bot.api_key

    unless @api_key.correct?
      redirect_to add_api_key_path
      return
    end

    prepare_step
    render_asset_page(bot: @bot, asset_field: :quote_asset_id) if paginate_asset_list?
  end

  def create
    if bot_params[:quote_asset_id].present?
      bot = build_bot
      session[:bot_config].deep_merge!({ settings: bot.parse_params(bot_params) }.deep_stringify_keys)
      after_quote_asset_picked
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

  def after_quote_asset_picked
    finalise_and_redirect
  end

  def finalise_and_redirect
    Bots::WizardDefaults.apply!(session[:bot_config]['settings'] ||= {}, wizard_default_settings)
    @bot = bot_relation.new(sanitized_bot_config.deep_symbolize_keys)
    @bot.set_missed_quote_amount
    if @bot.save
      session[:bot_config] = nil
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      prepare_step
      render :new, status: :unprocessable_entity
    end
  end

  # View state the :new template needs — shared by `new` and the 422 re-renders
  # in `create` (blank param) and `finalise_and_redirect` (failed save).
  def prepare_step
    @bot = build_bot
    @bot.quote_asset_id = nil
    @assets = asset_search_results(@bot, search_params[:query], :quote_asset)
  end

  # The index step renders its own (unpaginated) asset list.
  def paginate_asset_list? = true

  def redirect_if_session_expired
    render turbo_stream: turbo_stream_redirect(root_path) if session[:bot_config].blank?
  end

  def search_params
    params.permit(:query)
  end
end
