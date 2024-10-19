class SettingFlag < ApplicationRecord
  SHOW_ZEN_PAYMENT = 'show_zen_payment'.freeze
  SHOW_WIRE_PAYMENT = 'show_wire_payment'.freeze
  SHOW_BITCOIN_PAYMENT = 'show_bitcoin_payment'.freeze

  def self.show_zen_payment?
    @show_zen_payment ||= exists?(name: SHOW_ZEN_PAYMENT, value: true)
  end

  def self.show_bitcoin_payment?
    @show_bitcoin_payment ||= exists?(name: SHOW_BITCOIN_PAYMENT, value: true)
  end

  def self.show_wire_payment?
    @show_wire_payment ||= exists?(name: SHOW_WIRE_PAYMENT, value: true)
  end
end
