class Bots::Signals::ConfirmSettingsController < ApplicationController
  before_action :authenticate_user!

  def new
    session[:bot_config]['signals'] ||= [{ 'direction' => 'buy', 'amount' => 100, 'enabled' => true }]
    @bot = build_bot_with_signals
  end

  def create
    @bot = current_user.bots.signal.new(sanitized_bot_config)
    return render :create if @bot.valid?

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render :create, status: :unprocessable_entity
  end

  def add_signal
    session[:bot_config]['signals'] << { 'direction' => 'buy', 'amount' => 100, 'enabled' => true }
    @bot = build_bot_with_signals
    render turbo_stream: turbo_stream.replace('new-settings',
                                              partial: 'bots/signals/settings',
                                              locals: { bot: @bot })
  end

  def remove_signal
    index = params[:signal_index].to_i
    signals = session[:bot_config]['signals']
    signals.delete_at(index) if signals.size > 1
    @bot = build_bot_with_signals
    render turbo_stream: turbo_stream.replace('new-settings',
                                              partial: 'bots/signals/settings',
                                              locals: { bot: @bot })
  end

  def update_signal
    index = params[:signal_index].to_i
    signal = session[:bot_config]['signals'][index]
    return head :not_found unless signal

    signal['direction'] = params[:direction] if params[:direction].present?
    signal['amount'] = params[:amount].to_f if params[:amount].present?
    signal['amount_type'] = params[:amount_type] if params[:amount_type].present?
    if params.key?(:enabled)
      signal['enabled'] = params[:enabled] == '1'
      @bot = build_bot_with_signals
      return render turbo_stream: turbo_stream.replace('new-settings',
                                                       partial: 'bots/signals/settings',
                                                       locals: { bot: @bot })
    end
    head :ok
  end

  private

  def build_bot_with_signals
    bot = current_user.bots.signal.new(sanitized_bot_config)
    session[:bot_config]['signals'].each do |wh|
      bot.bot_signals.build(direction: wh['direction'], amount: wh['amount'], enabled: wh.fetch('enabled', true),
                            amount_type: wh.fetch('amount_type', 'fixed'))
    end
    bot
  end
end
