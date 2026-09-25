# The bot menu's index picker: "Follow index" on a portfolio, "Change the index" on an index bot.
# Offers only indices the bot's venue can host at the bot's spending currency (Index.followable_on).
class Bots::IndicesController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot
  before_action :require_composition_bot
  before_action :set_indices

  def new
    @assets_by_coingecko_id = Asset.where(external_id: @indices.flat_map { |index| index.top_coins || [] }.uniq)
                                   .index_by(&:external_id)
  end

  def create
    index = @indices.find { |candidate| candidate.external_id == params[:index_category_id] }
    return refuse(t('errors.bots.index_switch.not_offered')) if index.nil?

    bot = Bot::IndexSwitch.follow!(@bot, index)
    render turbo_stream: turbo_stream_redirect(bot_path(bot))
  rescue Bot::IndexSwitch::Refused => e
    refuse(e.message)
  end

  private

  def require_composition_bot
    head :not_found unless (@bot.dca_multi_asset? || @bot.dca_index?) && MarketData.configured?
  end

  # An index bot is not offered the index it already follows.
  def set_indices
    @indices = Index.followable_on(@bot.exchange, @bot.quote_asset_id)
    @indices = @indices.reject { |index| index.id == @bot.current_index&.id } if @bot.dca_index?
  end

  def refuse(message)
    flash.now[:alert] = message
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end
end
