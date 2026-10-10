require 'test_helper'

class LocaleKeysTest < ActiveSupport::TestCase
  # YAML 1.1 reads an unquoted yes:/no:/on:/off: key as a boolean, so I18n never finds it.
  test 'no locale file has a boolean key' do
    walk = lambda do |hash, path|
      hash.each do |key, value|
        assert_not [true, false].include?(key), "boolean key under #{path.join('.')}"
        walk.call(value, path + [key]) if value.is_a?(Hash)
      end
    end
    Rails.root.glob('config/locales/**/*.yml').each { |file| walk.call(YAML.load_file(file, aliases: true), [file.basename.to_s]) }
  end

  test 'the tax summary yes/no labels resolve' do
    assert_equal(%w[Yes No], %w[yes no].map { |k| I18n.t("tax_report.summary.#{k}", locale: :en) })
    assert_equal(%w[Ano Ne], %w[yes no].map { |k| I18n.t("tax_report.summary.#{k}", locale: :cs) })
  end

  test 'devise keys the app uses exist in every locale' do
    I18n.available_locales.each do |locale|
      %w[devise.sessions.two_factor.title devise.failure.locked].each do |key|
        assert I18n.exists?(key, locale, fallback: false), "missing #{key} in #{locale}"
      end
    end
  end
end
