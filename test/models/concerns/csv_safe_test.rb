require 'test_helper'

class CsvSafeTest < ActiveSupport::TestCase
  test 'neutralises a leading equals' do
    assert_equal "'=1+1", CsvSafe.cell('=1+1')
  end

  test 'neutralises the other formula leaders' do
    %w[+ - @].each { |c| assert_equal "'#{c}x", CsvSafe.cell("#{c}x") }
  end

  test 'neutralises leading control characters' do
    ["\t", "\r", "\n"].each { |c| assert_equal "'#{c}x", CsvSafe.cell("#{c}x") }
  end

  test 'neutralises a formula leader hidden behind leading whitespace' do
    assert_equal "' =1+1", CsvSafe.cell(' =1+1')
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

  test 'add_row and puts escape exactly like << instead of bypassing it' do
    csv = CsvSafe.generate do |out|
      out.add_row(['=evil'])
      out.puts(['=evil2'])
    end

    assert_includes csv, "'=evil\n"
    assert_includes csv, "'=evil2\n"
  end

  test 'unescape reverses a single leading quote' do
    assert_equal '=1+1', CsvSafe.unescape("'=1+1")
  end

  test 'unescape leaves a value with no escape prefix alone' do
    assert_equal 'BTC', CsvSafe.unescape('BTC')
  end

  test 'unescape passes non-strings through' do
    assert_nil CsvSafe.unescape(nil)
  end

  test 'a literal leading apostrophe round-trips through cell and unescape intact' do
    assert_equal "'abc", CsvSafe.unescape(CsvSafe.cell("'abc"))
  end

  test 'unescape leaves a literal leading apostrophe alone when nothing formula-leading follows' do
    assert_equal "'abc", CsvSafe.unescape("'abc")
  end

  test 'a formula-leading value still round-trips through cell and unescape intact' do
    assert_equal '=1+1', CsvSafe.unescape(CsvSafe.cell('=1+1'))
  end
end
