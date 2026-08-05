require 'test_helper'

class CsvSafeTest < ActiveSupport::TestCase
  test 'neutralises a leading equals' do
    assert_equal "'=1+1", CsvSafe.cell('=1+1')
  end

  test 'neutralises the other formula leaders' do
    %w[+ - @].each { |c| assert_equal "'#{c}x", CsvSafe.cell("#{c}x") }
  end

  test 'neutralises leading control characters' do
    assert_equal "'\tx", CsvSafe.cell("\tx")
  end

  test 'leaves ordinary text alone' do
    assert_equal 'BTC', CsvSafe.cell('BTC')
  end

  test 'leaves a negative number alone' do
    assert_equal(-1.5, CsvSafe.cell(-1.5))
  end

  test 'passes nil through' do
    assert_nil CsvSafe.cell(nil)
  end

  test 'generate escapes every row pushed through the writer' do
    csv = CsvSafe.generate do |out|
      out << ['header']
      out << ['=cmd|calc']
      out << []
    end

    assert_includes csv, "'=cmd|calc"
    refute_includes csv, "\n=cmd|calc"
  end
end
