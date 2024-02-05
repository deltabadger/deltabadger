class SettingFlagsRepository < BaseRepository
  SHOW_STRIPE_PAYMENT = 'show_stripe_payment'.freeze
  SHOW_ZEN_PAYMENT = 'show_zen_payment'.freeze
  SHOW_WIRE_PAYMENT = 'show_wire_payment'.freeze
  SHOW_BITCOIN_PAYMENT = 'show_bitcoin_payment'.freeze

  def model
    SettingFlag
  end

  def show_stripe_payment
    setting = model.find_by(name: SHOW_STRIPE_PAYMENT)
    return true if setting.nil?

    setting.value
  end

  def show_zen_payment
    setting = model.find_by(name: SHOW_ZEN_PAYMENT)
    return true if setting.nil?

    setting.value
  end

  def show_bitcoin_payment
    setting = model.find_by(name: SHOW_BITCOIN_PAYMENT)
    return false if setting.nil?

    setting.value
  end

  def show_wire_payment
    setting = model.find_by(name: SHOW_WIRE_PAYMENT)
    return false if setting.nil?

    setting.value
  end
end
