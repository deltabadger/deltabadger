class Bots::DcaDualAssetsController < ApplicationController
  before_action :authenticate_user!
  before_action :set_bot, only: %i[update show]
  before_action :set_new_bot, except: %i[update show]

  # Pick first asset to buy
  def new_step_one
    session[:bots_dca_dual_asset_params] = {}

    # TODO: move this block to a better place
    @bot.base0_asset_id = nil
    available_assets = @bot.available_assets_for_current_settings(asset_type: :base_asset)
    filtered_assets = filter_assets_by_query(assets: available_assets, query: bot_params[:query])
                      .pluck(:id, :symbol, :name)
    exchanges_data = Exchange.all.pluck(:id, :name_id, :name).each_with_object([]) do |(id, name_id, name), list|
      assets = Exchange.find(id).assets.pluck(:id)
      list << [name_id, name, assets] if assets.any?
    end
    @assets = filtered_assets.map do |id, symbol, name|
      exchanges = exchanges_data.select { |_, _, assets| assets.include?(id) }
      [id, symbol, name, exchanges.map { |e_name_id, e_name, _| [e_name_id, e_name] }]
    end
  end

  # Pick second asset to buy
  def new_step_two
    @bot.base1_asset_id = nil
    available_assets = @bot.available_assets_for_current_settings(asset_type: :base_asset)
    filtered_assets = filter_assets_by_query(assets: available_assets, query: bot_params[:query])
                      .pluck(:id, :symbol, :name)
    exchanges_data = Exchange.all.pluck(:id, :name_id, :name).each_with_object([]) do |(id, name_id, name), list|
      assets = Exchange.find(id).assets.pluck(:id)
      list << [name_id, name, assets] if assets.any?
    end
    @assets = filtered_assets.map do |id, symbol, name|
      exchanges = exchanges_data.select { |_, _, assets| assets.include?(id) }
      [id, symbol, name, exchanges.map { |e_name_id, e_name, _| [e_name_id, e_name] }]
    end
  end

  # Pick exchange
  def new_step_three
    @bot.exchange_id = nil
    exchanges = @bot.available_exchanges_for_current_settings
    @exchanges = filter_exchanges_by_query(exchanges: exchanges, query: bot_params[:query])
  end

  # Add API key
  def new_step_four
    @api_key = @bot.api_key
    @api_key.validate_key_permissions if @api_key.key.present? && @api_key.secret.present?
    redirect_to new_step_five_bots_dca_dual_assets_path if @api_key.correct?
  end

  # Create API key
  def create_step_four
    @api_key = @bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]
    @api_key.validate_key_permissions
    if @api_key.correct? && @api_key.save
      flash[:notice] = t('errors.bots.api_key_success')
      redirect_to new_step_five_bots_dca_dual_assets_path
    else
      render :new_step_four, status: :unprocessable_entity
    end
  end

  # Pick asset to spend
  def new_step_five
    @bot.quote_asset_id = nil
    available_assets = @bot.available_assets_for_current_settings(asset_type: :quote_asset)
    filtered_assets = filter_assets_by_query(assets: available_assets, query: bot_params[:query])
                      .pluck(:id, :symbol, :name)
    exchanges_data = Exchange.all.pluck(:id, :name_id, :name).each_with_object([]) do |(id, name_id, name), list|
      assets = Exchange.find(id).assets.pluck(:id)
      list << [name_id, name, assets] if assets.any?
    end
    @assets = filtered_assets.map do |id, symbol, name|
      exchanges = exchanges_data.select { |_, _, assets| assets.include?(id) }
      [id, symbol, name, exchanges.map { |e_name_id, e_name, _| [e_name_id, e_name] }]
    end
  end

  # Confirm settings
  def new
    @bot.interval ||= 'day'
    @bot.allocation0 ||= 0.5
    if @bot.quote_amount.blank? || @bot.valid?
      render :new
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render :new, status: :unprocessable_entity
    end
  end

  def create
    @bot.set_missed_quote_amount
    if @bot.save && @bot.start(start_fresh: true)
      session[:bots_dca_dual_asset_params] = nil
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      # FIXME: flash messages are not shown as they are rendered behind the modal
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  private

  def bot_params
    params.fetch(:bots_dca_dual_asset, {}).permit(
      :quote_amount,
      :quote_asset_id,
      :interval,
      :base0_asset_id,
      :base1_asset_id,
      :allocation0,
      :marketcap_allocated,
      :exchange_id,
      :label,
      :quote_amount_limited,
      :quote_amount_limit,
      :price_limited,
      :price_limit,
      :price_limit_timing_condition,
      :price_limit_price_condition,
      :price_limit_in_ticker_id,
      :smart_intervaled,
      :smart_interval_quote_amount,
      :query
    )
  end

  def api_key_params
    params.require(:api_key).permit(:key, :secret)
  end

  def bot_params_as_hash
    permitted_params = bot_params.to_h
    settings = permitted_params.except(:exchange_id, :label, :query).tap do |pp|
      pp[:base0_asset_id] = pp[:base0_asset_id].presence&.to_i
      pp[:base1_asset_id] = pp[:base1_asset_id].presence&.to_i
      pp[:quote_asset_id] = pp[:quote_asset_id].presence&.to_i
      pp[:quote_amount] = pp[:quote_amount].presence&.to_f
      pp[:allocation0] = pp[:allocation0].presence&.to_f
      pp[:marketcap_allocated] = pp[:marketcap_allocated].presence&.in?(%w[1 true])
      pp[:quote_amount_limited] = pp[:quote_amount_limited].presence&.in?(%w[1 true])
      pp[:quote_amount_limit] = pp[:quote_amount_limit].presence&.to_f
      pp[:price_limited] = pp[:price_limited].presence&.in?(%w[1 true])
      pp[:price_limit] = pp[:price_limit].presence&.to_f
      pp[:price_limit_timing_condition] = pp[:price_limit_timing_condition].presence
      pp[:price_limit_price_condition] = pp[:price_limit_price_condition].presence
      pp[:price_limit_in_ticker_id] = pp[:price_limit_in_ticker_id].presence&.to_i
      pp[:smart_intervaled] = pp[:smart_intervaled].presence&.in?(%w[1 true])
      pp[:smart_interval_quote_amount] = pp[:smart_interval_quote_amount].presence&.to_f
    end.compact
    {
      settings: settings,
      exchange_id: permitted_params[:exchange_id],
      label: permitted_params[:label].presence
    }.compact
  end

  def set_bot
    @bot = current_user.bots.find(params[:id])
  rescue ActiveRecord::RecordNotFound
    redirect_to bots_path, alert: t('bot.not_found')
  end

  def set_new_bot
    session[:bots_dca_dual_asset_params] ||= {}
    session[:bots_dca_dual_asset_params] =
      session[:bots_dca_dual_asset_params].deep_merge(bot_params_as_hash.deep_stringify_keys)
    @bot = current_user.bots.dca_dual_asset.new(session[:bots_dca_dual_asset_params])
    session[:bots_dca_dual_asset_params]['label'] = @bot.label
  end

  def filter_assets_by_query(assets:, query:)
    return assets.order(:market_cap_rank, :symbol) if query.blank?

    assets
      .map { |asset| [asset, similarities_for_asset(asset, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.7 }
      .sort_by { |asset, similarities| [similarities.map(&:-@), asset.market_cap_rank || Float::INFINITY] }
      .map(&:first)
  end

  def similarities_for_asset(asset, query)
    [
      asset.symbol.present? ? JaroWinkler.similarity(asset.symbol.downcase.to_s, query) : 0,
      asset.name.present? ? JaroWinkler.similarity(asset.name.downcase.to_s, query) : 0
    ].sort.reverse
  end

  def filter_exchanges_by_query(exchanges:, query:)
    return exchanges.order(:name) if query.blank?

    exchanges
      .map { |exchange| [exchange, similarities_for_exchange(exchange, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.7 }
      .sort_by { |_, similarities| similarities.map(&:-@) }
      .map(&:first)
  end

  def similarities_for_exchange(exchange, query)
    [
      exchange.name.present? ? JaroWinkler.similarity(exchange.name.downcase.to_s, query) : 0
    ].sort.reverse
  end
end
