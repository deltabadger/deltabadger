module.exports = {
  roots: ['app/javascript'],
  moduleDirectories: ['node_modules', 'app/javascript'],
  setupFilesAfterEnv: ['<rootDir>/app/javascript/setupTests.js'],
  testEnvironment: 'jsdom',
  transform: {
    '^.+\\.(js|jsx)$': 'babel-jest'
  },
  moduleNameMapper: {
    '\\.(css|less|sass|scss)$': '<rootDir>/app/javascript/__mocks__/styleMock.js',
    '\\.(gif|ttf|eot|svg)$': '<rootDir>/app/javascript/__mocks__/fileMock.js'
  }
} 