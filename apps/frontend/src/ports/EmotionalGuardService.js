/**
 * Emotional Trading Guard Service Port
 * 
 * Interface for emotional pattern detection.
 * Protects traders from impulsive decisions.
 */

export class EmotionalGuardService {
  /**
   * Analyze a trading intention for emotional patterns
   * @param {string} message - User's trading intention message
   * @returns {Promise<Object>} Analysis result with signals and response
   */
  async analyzeIntent(message) {
    throw new Error('Not implemented')
  }

  /**
   * Get list of all detectable signals
   * @returns {Promise<Object>} Signal types and descriptions
   */
  async listSignals() {
    throw new Error('Not implemented')
  }

  /**
   * Get trading psychology tips
   * @param {boolean} random - Return a random tip
   * @param {string} category - Filter by category
   * @returns {Promise<Object>} Tips list or single tip
   */
  async getTips(random = false, category = null) {
    throw new Error('Not implemented')
  }

  /**
   * Get risk level definitions
   * @returns {Promise<Object>} Risk levels with descriptions
   */
  async getRiskLevels() {
    throw new Error('Not implemented')
  }
}

