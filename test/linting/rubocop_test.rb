require 'test_helper'

class RubocopTest < ActiveSupport::TestCase
  test 'no rubocop offenses' do
    assert system('bundle exec rubocop --parallel', out: File::NULL, err: File::NULL), 'Rubocop offenses detected'
  end
end
