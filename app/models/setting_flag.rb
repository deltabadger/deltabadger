class SettingFlag < ApplicationRecord
  SHOW_ZEN_PAYMENT = 'show_zen_payment'.freeze
  SHOW_WIRE_PAYMENT = 'show_wire_payment'.freeze
  SHOW_BITCOIN_PAYMENT = 'show_bitcoin_payment'.freeze

  after_commit :reset_all_setting_flags_cache

  def self.show_zen_payment?
    all_setting_flags[SHOW_ZEN_PAYMENT] == true
  end

  def self.show_bitcoin_payment?
    all_setting_flags[SHOW_BITCOIN_PAYMENT] == true
  end

  def self.show_wire_payment?
    all_setting_flags[SHOW_WIRE_PAYMENT] == true
  end

  def self.all_setting_flags
    @all_setting_flags ||= all.map { |sf| [sf.name, sf.value] }.to_h
  end

  def self.reset_all_setting_flags_cache
    @all_setting_flags = nil
  end

  private

  def reset_all_setting_flags_cache
    self.class.reset_all_setting_flags_cache
  end
end
