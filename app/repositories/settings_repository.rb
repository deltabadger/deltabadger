class SettingsRepository
  SHOW_CARD_PAYMENT = 'show_card_payment'.freeze
  SHOW_WIRE_PAYMENT = 'show_wire_payment'.freeze
  SHOW_BITCOIN_PAYMENT = 'show_bitcoin_payment'.freeze

  def model
    Setting
  end

  def show_card_payment
    setting = model.find_by(name: SHOW_CARD_PAYMENT)
    return true if setting.nil?

    setting.value == 'true'
  end

  def show_bitcoin_payment
    setting = model.find_by(name: SHOW_BITCOIN_PAYMENT)
    return false if setting.nil?

    setting.value == 'true'
  end

  def show_wire_payment
    setting = model.find_by(name: SHOW_WIRE_PAYMENT)
    return false if setting.nil?

    setting.value == 'true'
  end
end