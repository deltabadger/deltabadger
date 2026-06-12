class Bots::SignalsController < Bots::Wizard::CreatesController
  private

  def bot_relation = current_user.bots.signal

  def build_bot
    signals_config = session.dig(:bot_config, 'signals') || [{ 'direction' => 'buy', 'amount' => 100 }]
    bot = super
    signals_config.each do |wh|
      bot.bot_signals.build(direction: wh['direction'], amount: wh['amount'], enabled: wh.fetch('enabled', true),
                            amount_type: wh.fetch('amount_type', 'fixed'))
    end
    bot
  end

  # Bots::Signal does not include Accountable — no missed-quote bookkeeping.
  def prepare_bot_for_save(_bot); end
end
